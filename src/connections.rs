use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

use crate::config::{Environment, EnvironmentKind};
use crate::utils::mongodb::mask_connection_string;

const SERVICE_NAME: &str = "arcula";
const CONFIG_FILE_NAME: &str = "connections.json";
const CONNECTION_VAULT_REF: &str = "connections-vault";

static VAULT_CACHE: OnceLock<Mutex<Option<ConnectionSecretVault>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionPolicy {
    #[serde(default = "default_true")]
    pub allow_as_source: bool,
    #[serde(default = "default_true")]
    pub allow_as_target: bool,
    #[serde(default = "default_true")]
    pub allow_agent_apply: bool,
    #[serde(default)]
    pub human_approval_required: bool,
    #[serde(default)]
    pub destructive_requires_backup: bool,
    #[serde(default)]
    pub backup_verification_required: bool,
}

impl ConnectionPolicy {
    pub fn for_kind(kind: EnvironmentKind, protected: bool) -> Self {
        let sensitive = protected || kind.is_prod();
        Self {
            allow_as_source: true,
            allow_as_target: true,
            allow_agent_apply: !sensitive,
            human_approval_required: sensitive,
            destructive_requires_backup: sensitive,
            backup_verification_required: sensitive,
        }
    }

    fn apply_safety_floor(&mut self, kind: EnvironmentKind, protected: bool) {
        if protected || kind.is_prod() {
            self.allow_agent_apply = false;
            self.human_approval_required = true;
            self.destructive_requires_backup = true;
            self.backup_verification_required = true;
        }
    }
}

impl Default for ConnectionPolicy {
    fn default() -> Self {
        Self::for_kind(EnvironmentKind::Other, false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectionMetadata {
    pub name: String,
    pub kind: EnvironmentKind,
    pub protected: bool,
    pub secret_ref: String,
    #[serde(default)]
    pub policy: ConnectionPolicy,
}

impl ConnectionMetadata {
    fn normalize(mut self) -> Self {
        self.protected = self.protected || self.kind.is_prod();
        self.policy.apply_safety_floor(self.kind, self.protected);
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConnectionPolicyPatch {
    pub allow_as_source: Option<bool>,
    pub allow_as_target: Option<bool>,
    pub allow_agent_apply: Option<bool>,
    pub human_approval_required: Option<bool>,
    pub destructive_requires_backup: Option<bool>,
    pub backup_verification_required: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub name: String,
    pub kind: EnvironmentKind,
    pub protected: bool,
    pub secret_ref: String,
    pub policy: ConnectionPolicy,
    pub uri_masked: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionMigrationReport {
    pub migrated: Vec<ConnectionInfo>,
    pub already_in_vault: Vec<String>,
    pub missing_legacy_secret: Vec<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ConnectionStore {
    #[serde(default)]
    connections: Vec<ConnectionMetadata>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ConnectionSecretVault {
    #[serde(default)]
    uris: BTreeMap<String, String>,
}

pub fn config_path() -> PathBuf {
    if let Ok(dir) = env::var("ARCULA_CONFIG_DIR") {
        return PathBuf::from(dir).join(CONFIG_FILE_NAME);
    }

    platform_config_dir().join("arcula").join(CONFIG_FILE_NAME)
}

pub fn normalize_name(name: &str) -> Result<String> {
    Ok(Environment::from_str(name)?.name().to_string())
}

pub fn list_metadata() -> Result<Vec<ConnectionMetadata>> {
    let mut connections: Vec<ConnectionMetadata> = load_store()?
        .connections
        .into_iter()
        .map(ConnectionMetadata::normalize)
        .collect();
    connections.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(connections)
}

pub fn list_infos(include_masked_uri: bool) -> Result<Vec<ConnectionInfo>> {
    list_metadata()?
        .into_iter()
        .map(|metadata| metadata_to_info(metadata, include_masked_uri))
        .collect()
}

pub fn get_info(name: &str, include_masked_uri: bool) -> Result<Option<ConnectionInfo>> {
    let Some(metadata) = find_metadata(name)? else {
        return Ok(None);
    };

    metadata_to_info(metadata, include_masked_uri).map(Some)
}

pub fn find_metadata(name: &str) -> Result<Option<ConnectionMetadata>> {
    let normalized = normalize_name(name)?;
    Ok(load_store()?
        .connections
        .into_iter()
        .map(ConnectionMetadata::normalize)
        .find(|connection| connection.name == normalized))
}

pub fn get_uri(name: &str) -> Result<Option<String>> {
    let Some(metadata) = find_metadata(name)? else {
        return Ok(None);
    };

    let mut vault = load_secret_vault()?;
    if let Some(uri) = vault.uris.get(&metadata.name) {
        return Ok(Some(uri.clone()));
    }

    // Backward compatibility: older Arcula versions stored one Keychain item per
    // connection. Lazily migrate the requested URI into the single vault so future
    // commands need at most one secure-storage access instead of one per connection.
    let entry = legacy_keyring_entry(&metadata.name)?;
    let uri = entry.get_password().with_context(|| {
        format!(
            "Failed to read URI for connection '{}' from secure storage",
            metadata.name
        )
    })?;
    vault.uris.insert(metadata.name.clone(), uri.clone());
    save_secret_vault(&vault)?;

    Ok(Some(uri))
}

pub fn upsert_connection(
    name: &str,
    uri: &str,
    kind: EnvironmentKind,
    protected: bool,
    force: bool,
) -> Result<ConnectionInfo> {
    validate_mongodb_uri(uri)?;
    let normalized = normalize_name(name)?;
    let protected = protected || kind.is_prod();
    let secret_ref_value = secret_ref(&normalized);

    let mut store = load_store()?;
    let existing_policy = store
        .connections
        .iter()
        .find(|connection| connection.name == normalized)
        .map(|connection| connection.clone().normalize().policy);
    let exists = existing_policy.is_some();

    if exists && !force {
        anyhow::bail!(
            "Connection '{}' already exists. Pass --force to update it.",
            normalized
        );
    }

    let mut vault = load_secret_vault()?;
    vault
        .uris
        .insert(normalized.clone(), uri.trim().to_string());
    save_secret_vault(&vault).with_context(|| {
        format!(
            "Failed to save URI for connection '{}' to secure storage",
            normalized
        )
    })?;

    store
        .connections
        .retain(|connection| connection.name != normalized);
    let metadata = ConnectionMetadata {
        name: normalized,
        kind,
        protected,
        secret_ref: secret_ref_value,
        policy: existing_policy.unwrap_or_else(|| ConnectionPolicy::for_kind(kind, protected)),
    }
    .normalize();
    store.connections.push(metadata.clone());
    save_store(&store)?;

    Ok(ConnectionInfo {
        name: metadata.name,
        kind: metadata.kind,
        protected: metadata.protected,
        secret_ref: metadata.secret_ref,
        policy: metadata.policy,
        uri_masked: Some(mask_connection_string(uri)),
    })
}

pub fn update_policy(name: &str, patch: ConnectionPolicyPatch) -> Result<ConnectionInfo> {
    let normalized = normalize_name(name)?;
    let mut store = load_store()?;
    let Some(metadata) = store
        .connections
        .iter_mut()
        .find(|connection| connection.name == normalized)
    else {
        anyhow::bail!("Connection '{}' does not exist", normalized);
    };

    if let Some(value) = patch.allow_as_source {
        metadata.policy.allow_as_source = value;
    }
    if let Some(value) = patch.allow_as_target {
        metadata.policy.allow_as_target = value;
    }
    if let Some(value) = patch.allow_agent_apply {
        metadata.policy.allow_agent_apply = value;
    }
    if let Some(value) = patch.human_approval_required {
        metadata.policy.human_approval_required = value;
    }
    if let Some(value) = patch.destructive_requires_backup {
        metadata.policy.destructive_requires_backup = value;
    }
    if let Some(value) = patch.backup_verification_required {
        metadata.policy.backup_verification_required = value;
    }

    let normalized_metadata = metadata.clone().normalize();
    *metadata = normalized_metadata.clone();
    save_store(&store)?;
    metadata_to_info(normalized_metadata, true)
}

pub fn remove_connection(name: &str) -> Result<ConnectionInfo> {
    let normalized = normalize_name(name)?;
    let mut store = load_store()?;
    let Some(index) = store
        .connections
        .iter()
        .position(|connection| connection.name == normalized)
    else {
        anyhow::bail!("Connection '{}' does not exist", normalized);
    };

    let metadata = store.connections.remove(index).normalize();
    let info = metadata_to_info(metadata.clone(), true)?;

    if let Ok(mut vault) = load_secret_vault() {
        vault.uris.remove(&normalized);
        let _ = save_secret_vault(&vault);
    }

    if let Ok(entry) = legacy_keyring_entry(&normalized) {
        let _ = entry.delete_credential();
    }

    save_store(&store)?;
    Ok(info)
}

pub fn migrate_legacy_connections_to_vault() -> Result<ConnectionMigrationReport> {
    let metadata = list_metadata()?;
    let mut vault = load_secret_vault()?;
    let mut migrated_metadata = Vec::new();
    let mut already_in_vault = Vec::new();
    let mut missing_legacy_secret = Vec::new();

    for connection in metadata {
        if vault.uris.contains_key(&connection.name) {
            already_in_vault.push(connection.name);
            continue;
        }

        let entry = legacy_keyring_entry(&connection.name)?;
        match entry.get_password() {
            Ok(uri) => {
                vault.uris.insert(connection.name.clone(), uri);
                migrated_metadata.push(connection);
            }
            Err(keyring::Error::NoEntry) => missing_legacy_secret.push(connection.name),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "Failed to read legacy URI for connection '{}' from secure storage",
                        connection.name
                    )
                });
            }
        }
    }

    if !migrated_metadata.is_empty() {
        save_secret_vault(&vault)?;
    }

    let migrated = migrated_metadata
        .into_iter()
        .map(|metadata| {
            let uri_masked = vault
                .uris
                .get(&metadata.name)
                .map(|uri| mask_connection_string(uri));
            ConnectionInfo {
                name: metadata.name,
                kind: metadata.kind,
                protected: metadata.protected,
                secret_ref: metadata.secret_ref,
                policy: metadata.policy,
                uri_masked,
            }
        })
        .collect();

    already_in_vault.sort();
    missing_legacy_secret.sort();

    Ok(ConnectionMigrationReport {
        migrated,
        already_in_vault,
        missing_legacy_secret,
    })
}

pub fn import_env_connections(force: bool) -> Result<Vec<ConnectionInfo>> {
    let mut imported = Vec::new();

    for (key, uri) in env::vars() {
        let Some(env_name) = key
            .strip_prefix("MONGO_")
            .and_then(|value| value.strip_suffix("_URI"))
        else {
            continue;
        };

        let env = Environment::from_str(env_name)?;
        let kind = env_kind_from_env(&env).unwrap_or_else(|| infer_environment_kind(env.name()));
        let protected = env_protected_from_env(&env).unwrap_or(false) || kind.is_prod();

        match upsert_connection(env.name(), &uri, kind, protected, force) {
            Ok(info) => imported.push(info),
            Err(err) if err.to_string().contains("already exists") && !force => {}
            Err(err) => return Err(err),
        }
    }

    imported.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(imported)
}

fn metadata_to_info(
    metadata: ConnectionMetadata,
    include_masked_uri: bool,
) -> Result<ConnectionInfo> {
    let metadata = metadata.normalize();
    let uri_masked = if include_masked_uri {
        get_uri(&metadata.name)?.map(|uri| mask_connection_string(&uri))
    } else {
        None
    };

    Ok(ConnectionInfo {
        name: metadata.name,
        kind: metadata.kind,
        protected: metadata.protected,
        secret_ref: metadata.secret_ref,
        policy: metadata.policy,
        uri_masked,
    })
}

fn load_store() -> Result<ConnectionStore> {
    let path = config_path();
    if !path.exists() {
        return Ok(ConnectionStore::default());
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read connection metadata from {}", path.display()))?;
    if contents.trim().is_empty() {
        return Ok(ConnectionStore::default());
    }

    serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse connection metadata in {}", path.display()))
}

fn save_store(store: &ConnectionStore) -> Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory {}", parent.display()))?;
    }

    let mut sorted = ConnectionStore {
        connections: store
            .connections
            .clone()
            .into_iter()
            .map(ConnectionMetadata::normalize)
            .collect(),
    };
    sorted.connections.sort_by(|a, b| a.name.cmp(&b.name));

    let contents = serde_json::to_string_pretty(&sorted)?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, contents).with_context(|| {
        format!(
            "Failed to write temporary config file {}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, &path).with_context(|| {
        format!(
            "Failed to move {} to {}",
            temp_path.display(),
            path.display()
        )
    })?;

    Ok(())
}

fn load_secret_vault() -> Result<ConnectionSecretVault> {
    let cache = VAULT_CACHE.get_or_init(|| Mutex::new(None));
    if let Some(vault) = cache
        .lock()
        .expect("connection vault cache mutex poisoned")
        .clone()
    {
        return Ok(vault);
    }

    let entry = vault_keyring_entry()?;
    let vault = match entry.get_password() {
        Ok(contents) => serde_json::from_str(&contents)
            .context("Failed to parse connection secret vault from secure storage")?,
        Err(keyring::Error::NoEntry) => ConnectionSecretVault::default(),
        Err(err) => return Err(err).context("Failed to read connection secret vault"),
    };

    *cache.lock().expect("connection vault cache mutex poisoned") = Some(vault.clone());
    Ok(vault)
}

fn save_secret_vault(vault: &ConnectionSecretVault) -> Result<()> {
    let contents = serde_json::to_string(vault)?;
    vault_keyring_entry()?
        .set_password(&contents)
        .context("Failed to save connection secret vault")?;
    *VAULT_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("connection vault cache mutex poisoned") = Some(vault.clone());
    Ok(())
}

fn vault_keyring_entry() -> Result<Entry> {
    Entry::new(SERVICE_NAME, CONNECTION_VAULT_REF).context("Failed to open secure credential store")
}

fn legacy_keyring_entry(name: &str) -> Result<Entry> {
    let user = secret_ref(name);
    Entry::new(SERVICE_NAME, &user).context("Failed to open secure credential store")
}

fn secret_ref(name: &str) -> String {
    format!("connection:{}", name.to_ascii_uppercase())
}

fn validate_mongodb_uri(uri: &str) -> Result<()> {
    let uri = uri.trim();
    if uri.is_empty() {
        anyhow::bail!("MongoDB URI cannot be empty");
    }
    if !(uri.starts_with("mongodb://") || uri.starts_with("mongodb+srv://")) {
        anyhow::bail!("MongoDB URI must start with mongodb:// or mongodb+srv://");
    }
    Ok(())
}

fn env_kind_from_env(env: &Environment) -> Option<EnvironmentKind> {
    for suffix in ["KIND", "TYPE", "ROLE"] {
        let var_name = format!("MONGO_{}_{}", env.name(), suffix);
        if let Ok(value) = env::var(var_name) {
            if let Ok(kind) = EnvironmentKind::from_str(&value) {
                return Some(kind);
            }
        }
    }
    None
}

fn env_protected_from_env(env: &Environment) -> Option<bool> {
    let var_name = format!("MONGO_{}_PROTECTED", env.name());
    env::var(var_name).ok().map(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn infer_environment_kind(name: &str) -> EnvironmentKind {
    match name.to_ascii_uppercase().as_str() {
        "LOCAL" | "LOC" => EnvironmentKind::Local,
        "DEV" | "DEVELOPMENT" => EnvironmentKind::Dev,
        "STG" | "STAGE" | "STAGING" => EnvironmentKind::Staging,
        "PROD" | "PRODUCTION" => EnvironmentKind::Prod,
        _ => EnvironmentKind::Other,
    }
}

fn platform_config_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support");
        }
    }

    if let Ok(xdg_config_home) = env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg_config_home);
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".config");
    }

    PathBuf::from(".")
}

fn default_true() -> bool {
    true
}
