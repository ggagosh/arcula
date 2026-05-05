use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::approvals::ApprovalRecord;
use crate::config::{
    get_connection_policy, get_environment_kind, is_protected_environment, EnvironmentKind,
    MongoConfig,
};
use crate::connections::ConnectionPolicy;
use crate::core::sync::SyncConfig;
use crate::storage;

const PLAN_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Planned,
    Approved,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPlanRecord {
    pub version: u8,
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: PlanStatus,
    pub hash: String,
    pub policy_hash: String,
    pub config: SyncConfig,
    pub source_kind: EnvironmentKind,
    pub target_kind: EnvironmentKind,
    pub source_protected: bool,
    pub target_protected: bool,
    pub source_policy: ConnectionPolicy,
    pub target_policy: ConnectionPolicy,
    pub destructive: bool,
    pub requires_full_backup: bool,
    pub requires_human_approval: bool,
    pub warnings: Vec<String>,
}

#[derive(Serialize)]
struct PlanHashPayload<'a> {
    version: u8,
    config: &'a SyncConfig,
    source_kind: EnvironmentKind,
    target_kind: EnvironmentKind,
    source_protected: bool,
    target_protected: bool,
    source_policy: &'a ConnectionPolicy,
    target_policy: &'a ConnectionPolicy,
    destructive: bool,
    requires_full_backup: bool,
    requires_human_approval: bool,
    warnings: &'a [String],
}

#[derive(Serialize)]
struct PolicyHashPayload<'a> {
    source_policy: &'a ConnectionPolicy,
    target_policy: &'a ConnectionPolicy,
    source_kind: EnvironmentKind,
    target_kind: EnvironmentKind,
    source_protected: bool,
    target_protected: bool,
}

pub fn create_sync_plan(mut config: SyncConfig) -> Result<SyncPlanRecord> {
    config.options.update_collection_settings();
    crate::utils::mongodb::validate_db_name(&config.source_db)?;
    crate::utils::mongodb::validate_db_name(&config.target_db)?;
    let _source_config = MongoConfig::from_env(config.source_env.clone()).with_context(|| {
        format!(
            "Source connection '{}' is not configured",
            config.source_env
        )
    })?;
    let _target_config = MongoConfig::from_env(config.target_env.clone()).with_context(|| {
        format!(
            "Target connection '{}' is not configured",
            config.target_env
        )
    })?;

    let source_kind = get_environment_kind(&config.source_env);
    let target_kind = get_environment_kind(&config.target_env);
    let source_protected = is_protected_environment(&config.source_env);
    let target_protected = is_protected_environment(&config.target_env);
    let source_policy = get_connection_policy(&config.source_env);
    let target_policy = get_connection_policy(&config.target_env);
    let destructive = config.options.is_destructive();
    let requires_full_backup = destructive && target_policy.destructive_requires_backup;
    let requires_human_approval = target_policy.human_approval_required;

    if !source_policy.allow_as_source {
        anyhow::bail!(
            "Connection '{}' is not allowed as a sync source",
            config.source_env
        );
    }
    if !target_policy.allow_as_target {
        anyhow::bail!(
            "Connection '{}' is not allowed as a sync target",
            config.target_env
        );
    }
    if requires_full_backup && !config.options.create_backup {
        anyhow::bail!(
            "Refusing destructive sync to protected/production target '{}:{}' without a full backup. Set --backup true.",
            config.target_env,
            config.target_db
        );
    }

    let mut warnings = Vec::new();
    if config.source_env == config.target_env {
        warnings.push("Source and target connections are the same".to_string());
    }
    if destructive {
        warnings.push("Target collections will be dropped or cleared before import".to_string());
    }
    if target_protected || target_kind.is_prod() {
        warnings.push("Target is protected/production".to_string());
    }
    if requires_human_approval {
        warnings.push("Human OS approval is required before execution".to_string());
    }

    let now = chrono::Utc::now().to_rfc3339();
    let mut plan = SyncPlanRecord {
        version: PLAN_VERSION,
        id: generate_plan_id(),
        created_at: now.clone(),
        updated_at: now,
        status: PlanStatus::Planned,
        hash: String::new(),
        policy_hash: String::new(),
        config,
        source_kind,
        target_kind,
        source_protected,
        target_protected,
        source_policy,
        target_policy,
        destructive,
        requires_full_backup,
        requires_human_approval,
        warnings,
    };
    plan.hash = compute_plan_hash(&plan)?;
    plan.policy_hash = compute_policy_hash(&plan)?;
    Ok(plan)
}

pub fn save_plan(plan: &SyncPlanRecord) -> Result<()> {
    storage::atomic_write_json(&plan_path(&plan.id), plan)
}

pub fn create_and_save_sync_plan(config: SyncConfig) -> Result<SyncPlanRecord> {
    let plan = create_sync_plan(config)?;
    save_plan(&plan)?;
    Ok(plan)
}

pub fn load_plan(id: &str) -> Result<SyncPlanRecord> {
    let path = plan_path(id);
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read plan {} from {}", id, path.display()))?;
    let plan: SyncPlanRecord =
        serde_json::from_str(&contents).with_context(|| format!("Failed to parse plan {}", id))?;
    let expected_hash = compute_plan_hash(&plan)?;
    if plan.hash != expected_hash {
        anyhow::bail!(
            "Plan '{}' hash mismatch. Expected {}, found {}. Refusing to use possibly edited plan.",
            id,
            expected_hash,
            plan.hash
        );
    }
    Ok(plan)
}

pub fn list_plans() -> Result<Vec<SyncPlanRecord>> {
    let dir = storage::plans_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut plans = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path().join("plan.json");
        if path.exists() {
            let contents = fs::read_to_string(&path)?;
            let plan: SyncPlanRecord = serde_json::from_str(&contents)?;
            plans.push(plan);
        }
    }
    plans.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(plans)
}

pub fn update_plan_status(id: &str, status: PlanStatus) -> Result<SyncPlanRecord> {
    let mut plan = load_plan(id)?;
    plan.status = status;
    plan.updated_at = chrono::Utc::now().to_rfc3339();
    save_plan(&plan)?;
    Ok(plan)
}

pub fn save_approval(plan_id: &str, approval: &ApprovalRecord) -> Result<()> {
    storage::atomic_write_json(&approval_path(plan_id), approval)
}

pub fn load_approval(plan_id: &str) -> Result<Option<ApprovalRecord>> {
    let path = approval_path(plan_id);
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read approval from {}", path.display()))?;
    let approval = serde_json::from_str(&contents)
        .with_context(|| format!("Failed to parse approval for plan {plan_id}"))?;
    Ok(Some(approval))
}

pub fn render_plan_text(plan: &SyncPlanRecord) -> String {
    let mut text = String::new();
    text.push_str(&format!("Plan: {}\n", plan.id));
    text.push_str(&format!("Status: {:?}\n", plan.status));
    text.push_str(&format!("Hash: {}\n", plan.hash));
    text.push_str(&format!(
        "Source: {}:{} ({})\n",
        plan.config.source_env, plan.config.source_db, plan.source_kind
    ));
    text.push_str(&format!(
        "Target: {}:{} ({})\n",
        plan.config.target_env, plan.config.target_db, plan.target_kind
    ));
    text.push_str(&format!("Target protected: {}\n", plan.target_protected));
    text.push_str(&format!("Backup: {}\n", plan.config.options.create_backup));
    text.push_str(&format!("Drop: {}\n", plan.config.options.drop_collections));
    text.push_str(&format!(
        "Clear: {}\n",
        plan.config.options.clear_collections
    ));
    text.push_str(&format!(
        "Requires human approval: {}\n",
        plan.requires_human_approval
    ));
    text.push_str(&format!(
        "Requires full backup: {}\n",
        plan.requires_full_backup
    ));
    if !plan.warnings.is_empty() {
        text.push_str("Warnings:\n");
        for warning in &plan.warnings {
            text.push_str(&format!("  - {warning}\n"));
        }
    }
    text
}

pub fn render_plan_markdown(plan: &SyncPlanRecord) -> String {
    let mut md = String::new();
    md.push_str(&format!("# Arcula sync plan `{}`\n\n", plan.id));
    md.push_str(&format!("- **Status:** `{:?}`\n", plan.status));
    md.push_str(&format!("- **Plan hash:** `{}`\n", plan.hash));
    md.push_str(&format!(
        "- **Source:** `{}` / `{}` / `{}`\n",
        plan.config.source_env, plan.config.source_db, plan.source_kind
    ));
    md.push_str(&format!(
        "- **Target:** `{}` / `{}` / `{}`\n",
        plan.config.target_env, plan.config.target_db, plan.target_kind
    ));
    md.push_str(&format!(
        "- **Target protected:** `{}`\n",
        plan.target_protected
    ));
    md.push_str(&format!(
        "- **Backup:** `{}`\n",
        plan.config.options.create_backup
    ));
    md.push_str(&format!(
        "- **Drop:** `{}`\n",
        plan.config.options.drop_collections
    ));
    md.push_str(&format!(
        "- **Clear:** `{}`\n",
        plan.config.options.clear_collections
    ));
    md.push_str(&format!(
        "- **Requires human approval:** `{}`\n",
        plan.requires_human_approval
    ));
    md.push_str(&format!(
        "- **Requires full backup:** `{}`\n",
        plan.requires_full_backup
    ));
    if !plan.warnings.is_empty() {
        md.push_str("\n## Warnings\n\n");
        for warning in &plan.warnings {
            md.push_str(&format!("- {warning}\n"));
        }
    }
    md
}

pub fn plan_dir(id: &str) -> PathBuf {
    storage::plans_dir().join(id)
}

fn plan_path(id: &str) -> PathBuf {
    plan_dir(id).join("plan.json")
}

fn approval_path(plan_id: &str) -> PathBuf {
    plan_dir(plan_id).join("approval.json")
}

fn compute_plan_hash(plan: &SyncPlanRecord) -> Result<String> {
    let payload = PlanHashPayload {
        version: plan.version,
        config: &plan.config,
        source_kind: plan.source_kind,
        target_kind: plan.target_kind,
        source_protected: plan.source_protected,
        target_protected: plan.target_protected,
        source_policy: &plan.source_policy,
        target_policy: &plan.target_policy,
        destructive: plan.destructive,
        requires_full_backup: plan.requires_full_backup,
        requires_human_approval: plan.requires_human_approval,
        warnings: &plan.warnings,
    };
    hash_json(&payload)
}

fn compute_policy_hash(plan: &SyncPlanRecord) -> Result<String> {
    let payload = PolicyHashPayload {
        source_policy: &plan.source_policy,
        target_policy: &plan.target_policy,
        source_kind: plan.source_kind,
        target_kind: plan.target_kind,
        source_protected: plan.source_protected,
        target_protected: plan.target_protected,
    };
    hash_json(&payload)
}

fn hash_json<T: Serialize>(value: &T) -> Result<String> {
    let bytes = serde_json::to_vec(value)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(to_hex(&hasher.finalize()))
}

fn generate_plan_id() -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let suffix: u32 = rand::random();
    format!("sync_{timestamp}_{suffix:08x}")
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}
