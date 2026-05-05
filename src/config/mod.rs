use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::ValueEnum;
use mongodb::options::ClientOptions;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Environment variable not found: {0}")]
    EnvVarNotFound(String),

    #[error("Invalid environment: {0}")]
    InvalidEnvironment(String),

    #[error("MongoDB connection error: {0}")]
    MongoDBConnection(#[from] mongodb::error::Error),

    #[error("Failed to locate MongoDB binary: {0}")]
    WhichError(#[from] which::Error),

    #[error("MongoDB binary not found")]
    BinaryNotFound,

    #[error("Connection store error: {0}")]
    ConnectionStore(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Environment(String);

impl Environment {
    pub fn new(name: &str) -> Self {
        Self(name.to_uppercase())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Environment {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let name = s.trim();
        if name.is_empty() {
            return Err(ConfigError::InvalidEnvironment(
                "Empty environment name".to_string(),
            ));
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(ConfigError::InvalidEnvironment(format!(
                "Environment names may only contain letters, numbers, and underscores: {name}"
            )));
        }
        Ok(Self::new(name))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentKind {
    Local,
    Dev,
    Staging,
    Prod,
    Other,
}

impl fmt::Display for EnvironmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Dev => write!(f, "dev"),
            Self::Staging => write!(f, "staging"),
            Self::Prod => write!(f, "prod"),
            Self::Other => write!(f, "other"),
        }
    }
}

impl FromStr for EnvironmentKind {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" | "localhost" => Ok(Self::Local),
            "dev" | "development" => Ok(Self::Dev),
            "stg" | "stage" | "staging" => Ok(Self::Staging),
            "prod" | "production" => Ok(Self::Prod),
            "other" | "unknown" => Ok(Self::Other),
            value => Err(ConfigError::InvalidEnvironment(format!(
                "Invalid environment kind: {value}"
            ))),
        }
    }
}

impl EnvironmentKind {
    pub fn is_prod(self) -> bool {
        matches!(self, Self::Prod)
    }
}

#[derive(Debug, Clone)]
pub struct MongoConfig {
    pub connection_string: String,
    pub environment: Environment,
}

impl MongoConfig {
    pub fn from_env(env: Environment) -> Result<Self, ConfigError> {
        if let Some(connection_string) = crate::connections::get_uri(env.name())
            .map_err(|err| ConfigError::ConnectionStore(err.to_string()))?
        {
            return Ok(Self {
                connection_string,
                environment: env,
            });
        }

        let var_name = format!("MONGO_{}_URI", env);
        let connection_string =
            env::var(&var_name).map_err(|_| ConfigError::EnvVarNotFound(var_name))?;

        Ok(Self {
            connection_string,
            environment: env,
        })
    }

    pub async fn get_client_options(&self) -> Result<ClientOptions, ConfigError> {
        let options = ClientOptions::parse(&self.connection_string).await?;
        Ok(options)
    }
}

pub fn get_mongodb_bin_path() -> Result<PathBuf, ConfigError> {
    if let Ok(path) = env::var("MONGODB_BIN_PATH") {
        let path_buf = PathBuf::from(&path);
        let mongodump_exists = path_buf.join("mongodump").exists();
        let mongorestore_exists = path_buf.join("mongorestore").exists();

        if mongodump_exists && mongorestore_exists {
            return Ok(path_buf);
        }

        let mut missing = Vec::new();
        if !mongodump_exists {
            missing.push("mongodump");
        }
        if !mongorestore_exists {
            missing.push("mongorestore");
        }

        return Err(ConfigError::InvalidEnvironment(format!(
            "MONGODB_BIN_PATH='{}' missing: {}",
            path,
            missing.join(", ")
        )));
    }

    // Try to find mongodump in PATH using 'which'
    if let Ok(mongodump_path) = which::which("mongodump") {
        if let Some(parent) = mongodump_path.parent() {
            // Verify mongorestore exists in the same directory
            if parent.join("mongorestore").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }

    // If we get here, we couldn't find the binaries
    Err(ConfigError::BinaryNotFound)
}

/// Checks if MongoDB tools (mongodump and mongorestore) are available
pub fn check_mongodb_tools() -> Result<(), ConfigError> {
    // This will return an error if it can't find both mongodump and mongorestore
    get_mongodb_bin_path().map(|_| ())
}

/// Get all available MongoDB environments from environment variables
pub fn get_environment_kind(env: &Environment) -> EnvironmentKind {
    let inferred = infer_environment_kind(env.name());
    if inferred.is_prod() {
        return EnvironmentKind::Prod;
    }

    for suffix in ["KIND", "TYPE", "ROLE"] {
        let var_name = format!("MONGO_{}_{}", env.name(), suffix);
        if let Ok(value) = env::var(var_name) {
            if let Ok(kind) = <EnvironmentKind as FromStr>::from_str(&value) {
                return kind;
            }
        }
    }

    if let Ok(Some(metadata)) = crate::connections::find_metadata(env.name()) {
        return metadata.kind;
    }

    inferred
}

pub fn get_connection_policy(env: &Environment) -> crate::connections::ConnectionPolicy {
    if let Ok(Some(metadata)) = crate::connections::find_metadata(env.name()) {
        return metadata.policy;
    }

    crate::connections::ConnectionPolicy::for_kind(
        get_environment_kind(env),
        is_protected_environment(env),
    )
}

pub fn is_protected_environment(env: &Environment) -> bool {
    if get_environment_kind(env).is_prod() {
        return true;
    }

    if let Ok(Some(metadata)) = crate::connections::find_metadata(env.name()) {
        if metadata.protected {
            return true;
        }
    }

    let var_name = format!("MONGO_{}_PROTECTED", env.name());
    env::var(var_name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
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

/// Get all available MongoDB environments from environment variables
pub fn get_available_environments() -> Vec<Environment> {
    let prefix = "MONGO_";
    let suffix = "_URI";

    let mut names = BTreeSet::new();

    for (key, _) in env::vars() {
        if key.starts_with(prefix) && key.ends_with(suffix) {
            if let Some(env_name) = key
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
            {
                if !env_name.is_empty() {
                    names.insert(Environment::new(env_name).name().to_string());
                }
            }
        }
    }

    if let Ok(connections) = crate::connections::list_metadata() {
        for connection in connections {
            names.insert(connection.name);
        }
    }

    names
        .into_iter()
        .map(|name| Environment::new(&name))
        .collect()
}

pub fn get_backup_dir() -> PathBuf {
    env::var("BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut path = env::temp_dir();
            path.push("mongo_importer_backups");
            path
        })
}
