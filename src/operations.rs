use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::approvals::{self, ApprovalMode, ApprovalRecord};
use crate::config::{get_connection_policy, MongoConfig};
use crate::core::sync::{perform_sync, SyncReport};
use crate::plans::{self, PlanStatus, SyncPlanRecord};
use crate::storage;
use crate::utils::mongodb;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Sync,
    Revert,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStatusEvent {
    pub at: String,
    pub status: OperationStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub version: u8,
    pub id: String,
    pub kind: OperationKind,
    pub plan_id: Option<String>,
    pub plan_hash: Option<String>,
    pub original_operation_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub status: OperationStatus,
    pub status_history: Vec<OperationStatusEvent>,
    pub approval: Option<ApprovalRecord>,
    pub sync_report: Option<SyncReport>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RevertPreview {
    pub operation_id: String,
    pub target_env: String,
    pub target_db: String,
    pub backup_path: String,
    pub target_policy_requires_human_approval: bool,
}

pub async fn run_plan(plan_id: &str, agent: bool) -> Result<OperationRecord> {
    let plan = plans::load_plan(plan_id)?;
    if matches!(plan.status, PlanStatus::Running | PlanStatus::Completed) {
        anyhow::bail!(
            "Plan '{}' is {:?} and cannot be run again. Create a new plan instead.",
            plan.id,
            plan.status
        );
    }
    let approval = ensure_plan_approval(&plan, agent)?;

    let mut operation = OperationRecord::new_sync(&plan, approval.clone());
    save_operation(&operation)?;
    plans::update_plan_status(&plan.id, PlanStatus::Running)?;

    match perform_sync(plan.config.clone()).await {
        Ok(report) => {
            operation.sync_report = Some(report);
            operation.push_status(OperationStatus::Completed, "sync completed");
            save_operation(&operation)?;
            plans::update_plan_status(&plan.id, PlanStatus::Completed)?;
            Ok(operation)
        }
        Err(err) => {
            operation.error = Some(format!("{err:#}"));
            operation.push_status(OperationStatus::Failed, "sync failed");
            save_operation(&operation)?;
            let _ = plans::update_plan_status(&plan.id, PlanStatus::Failed);
            Err(err)
        }
    }
}

pub fn preview_revert(operation_id: &str) -> Result<RevertPreview> {
    let operation = load_operation(operation_id)?;
    let report = operation
        .sync_report
        .as_ref()
        .context("Operation has no sync report and cannot be reverted")?;
    let backup_path = report
        .backup_path
        .clone()
        .context("Operation has no backup path and cannot be reverted")?;
    if !std::path::Path::new(&backup_path).exists() {
        anyhow::bail!("Backup path does not exist: {backup_path}");
    }
    let target_policy = get_connection_policy(&report.target_env);

    Ok(RevertPreview {
        operation_id: operation.id,
        target_env: report.target_env.name().to_string(),
        target_db: report.target_db.clone(),
        backup_path,
        target_policy_requires_human_approval: target_policy.human_approval_required,
    })
}

pub async fn revert_operation(operation_id: &str) -> Result<OperationRecord> {
    crate::config::check_mongodb_tools()
        .map_err(|err| anyhow::anyhow!("MongoDB tools not found: {err}"))?;

    let original = load_operation(operation_id)?;
    let report = original
        .sync_report
        .as_ref()
        .context("Operation has no sync report and cannot be reverted")?;
    let backup_path = report
        .backup_path
        .clone()
        .context("Operation has no backup path and cannot be reverted")?;
    if !std::path::Path::new(&backup_path).exists() {
        anyhow::bail!("Backup path does not exist: {backup_path}");
    }

    let target_policy = get_connection_policy(&report.target_env);
    let approval = if target_policy.human_approval_required {
        Some(approvals::create_human_presence_approval(
            &format!("revert:{}", original.id),
            &revert_hash(&original)?,
        )?)
    } else {
        Some(approvals::create_agent_policy_approval(
            &format!("revert:{}", original.id),
            &revert_hash(&original)?,
        )?)
    };

    let mut operation = OperationRecord::new_revert(&original, approval);
    save_operation(&operation)?;

    let target_config = MongoConfig::from_env(report.target_env.clone()).context(format!(
        "Failed to get configuration for {}",
        report.target_env
    ))?;
    match mongodb::restore_backup(
        &target_config,
        &report.target_db,
        std::path::Path::new(&backup_path),
    )
    .await
    {
        Ok(()) => {
            let mut revert_report = report.clone();
            revert_report.restored_from_backup = true;
            operation.sync_report = Some(revert_report);
            operation.push_status(OperationStatus::Completed, "revert completed");
            save_operation(&operation)?;
            Ok(operation)
        }
        Err(err) => {
            operation.error = Some(format!("{err:#}"));
            operation.push_status(OperationStatus::Failed, "revert failed");
            save_operation(&operation)?;
            Err(err)
        }
    }
}

pub fn load_operation(id: &str) -> Result<OperationRecord> {
    let path = operation_path(id);
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read operation {} from {}", id, path.display()))?;
    serde_json::from_str(&contents).with_context(|| format!("Failed to parse operation {id}"))
}

pub fn list_operations() -> Result<Vec<OperationRecord>> {
    let dir = storage::operations_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut operations = Vec::new();
    for entry in fs::read_dir(&dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path().join("operation.json");
        if path.exists() {
            let contents = fs::read_to_string(&path)?;
            let operation: OperationRecord = serde_json::from_str(&contents)?;
            operations.push(operation);
        }
    }
    operations.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(operations)
}

fn ensure_plan_approval(plan: &SyncPlanRecord, agent: bool) -> Result<Option<ApprovalRecord>> {
    if let Some(approval) = plans::load_approval(&plan.id)? {
        approvals::verify_approval(&approval, &plan.id, &plan.hash)?;
        if plan.requires_human_approval && approval.mode != ApprovalMode::HumanPresence {
            anyhow::bail!("Plan '{}' requires human OS approval", plan.id);
        }
        return Ok(Some(approval));
    }

    if plan.requires_human_approval {
        anyhow::bail!(
            "Plan '{}' requires human approval. Run: arcula plan approve {}",
            plan.id,
            plan.id
        );
    }

    if agent && !plan.target_policy.allow_agent_apply {
        anyhow::bail!(
            "Target policy does not allow agent execution for plan '{}'",
            plan.id
        );
    }

    let approval = approvals::create_agent_policy_approval(&plan.id, &plan.hash)?;
    plans::save_approval(&plan.id, &approval)?;
    plans::update_plan_status(&plan.id, PlanStatus::Approved)?;
    Ok(Some(approval))
}

fn save_operation(operation: &OperationRecord) -> Result<()> {
    storage::atomic_write_json(&operation_path(&operation.id), operation)
}

fn operation_dir(id: &str) -> PathBuf {
    storage::operations_dir().join(id)
}

fn operation_path(id: &str) -> PathBuf {
    operation_dir(id).join("operation.json")
}

fn revert_hash(operation: &OperationRecord) -> Result<String> {
    let bytes = serde_json::to_vec(operation)?;
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(to_hex(&hasher.finalize()))
}

fn generate_operation_id(prefix: &str) -> String {
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let suffix: u32 = rand::random();
    format!("{prefix}_{timestamp}_{suffix:08x}")
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl OperationRecord {
    fn new_sync(plan: &SyncPlanRecord, approval: Option<ApprovalRecord>) -> Self {
        let created_at = now();
        let mut operation = Self {
            version: 1,
            id: generate_operation_id("op"),
            kind: OperationKind::Sync,
            plan_id: Some(plan.id.clone()),
            plan_hash: Some(plan.hash.clone()),
            original_operation_id: None,
            created_at: created_at.clone(),
            updated_at: created_at,
            status: OperationStatus::Running,
            status_history: Vec::new(),
            approval,
            sync_report: None,
            error: None,
        };
        operation.push_status(OperationStatus::Running, "sync started");
        operation
    }

    fn new_revert(original: &OperationRecord, approval: Option<ApprovalRecord>) -> Self {
        let created_at = now();
        let mut operation = Self {
            version: 1,
            id: generate_operation_id("revert"),
            kind: OperationKind::Revert,
            plan_id: original.plan_id.clone(),
            plan_hash: original.plan_hash.clone(),
            original_operation_id: Some(original.id.clone()),
            created_at: created_at.clone(),
            updated_at: created_at,
            status: OperationStatus::Running,
            status_history: Vec::new(),
            approval,
            sync_report: None,
            error: None,
        };
        operation.push_status(OperationStatus::Running, "revert started");
        operation
    }

    fn push_status(&mut self, status: OperationStatus, message: &str) {
        self.status = status.clone();
        self.updated_at = now();
        self.status_history.push(OperationStatusEvent {
            at: self.updated_at.clone(),
            status,
            message: message.to_string(),
        });
    }
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
