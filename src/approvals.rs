use std::fs;
use std::io::IsTerminal;
use std::process::{Command, Stdio};

use anyhow::{anyhow, Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SERVICE_NAME: &str = "arcula";
const APPROVAL_SECRET_REF: &str = "approval-signing-key";
const APPROVAL_TTL_SECONDS: i64 = 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    AgentPolicy,
    HumanPresence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRecord {
    pub plan_id: String,
    pub plan_hash: String,
    pub approved_at: String,
    pub expires_at: String,
    pub provider: String,
    pub approved_by: String,
    pub mode: ApprovalMode,
    pub signature: String,
}

impl ApprovalRecord {
    pub fn is_expired(&self) -> bool {
        parse_timestamp(&self.expires_at)
            .map(|expires_at| chrono::Utc::now() > expires_at)
            .unwrap_or(true)
    }
}

pub fn create_agent_policy_approval(plan_id: &str, plan_hash: &str) -> Result<ApprovalRecord> {
    create_signed_approval(
        plan_id,
        plan_hash,
        ApprovalMode::AgentPolicy,
        "agent_policy",
    )
}

pub fn create_human_presence_approval(plan_id: &str, plan_hash: &str) -> Result<ApprovalRecord> {
    require_user_presence(plan_id, plan_hash)?;
    create_signed_approval(
        plan_id,
        plan_hash,
        ApprovalMode::HumanPresence,
        platform_provider(),
    )
}

pub fn verify_approval(record: &ApprovalRecord, plan_id: &str, plan_hash: &str) -> Result<()> {
    if record.plan_id != plan_id {
        anyhow::bail!(
            "Approval is for plan '{}' but operation requires plan '{}'",
            record.plan_id,
            plan_id
        );
    }
    if record.plan_hash != plan_hash {
        anyhow::bail!("Approval does not match the current plan hash");
    }
    if record.is_expired() {
        anyhow::bail!("Approval expired at {}", record.expires_at);
    }

    let expected = approval_signature(record, &approval_secret()?)?;
    if record.signature != expected {
        anyhow::bail!("Approval signature is invalid");
    }

    Ok(())
}

fn create_signed_approval(
    plan_id: &str,
    plan_hash: &str,
    mode: ApprovalMode,
    provider: &str,
) -> Result<ApprovalRecord> {
    let approved_at_time = chrono::Utc::now();
    let expires_at_time = approved_at_time + chrono::Duration::seconds(APPROVAL_TTL_SECONDS);
    let mut record = ApprovalRecord {
        plan_id: plan_id.to_string(),
        plan_hash: plan_hash.to_string(),
        approved_at: approved_at_time.to_rfc3339(),
        expires_at: expires_at_time.to_rfc3339(),
        provider: provider.to_string(),
        approved_by: current_user(),
        mode,
        signature: String::new(),
    };
    record.signature = approval_signature(&record, &approval_secret()?)?;
    Ok(record)
}

fn approval_signature(record: &ApprovalRecord, secret: &str) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.plan_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.plan_hash.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.approved_at.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.expires_at.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.provider.as_bytes());
    hasher.update(b"\n");
    hasher.update(record.approved_by.as_bytes());
    hasher.update(b"\n");
    hasher.update(serde_json::to_string(&record.mode)?.as_bytes());
    Ok(to_hex(&hasher.finalize()))
}

fn approval_secret() -> Result<String> {
    if let Ok(entry) = Entry::new(SERVICE_NAME, APPROVAL_SECRET_REF) {
        if let Ok(secret) = entry.get_password() {
            return Ok(secret);
        }
    }

    let fallback_path = crate::storage::data_dir().join("approval-signing-key");
    if fallback_path.exists() {
        return fs::read_to_string(&fallback_path)
            .map(|value| value.trim().to_string())
            .with_context(|| format!("Failed to read {}", fallback_path.display()));
    }

    let bytes: [u8; 32] = rand::random();
    let secret = to_hex(&bytes);

    if let Ok(entry) = Entry::new(SERVICE_NAME, APPROVAL_SECRET_REF) {
        let _ = entry.set_password(&secret);
    }

    if let Some(parent) = fallback_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(&fallback_path, &secret)
        .with_context(|| format!("Failed to write {}", fallback_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&fallback_path, fs::Permissions::from_mode(0o600));
    }

    Ok(secret)
}

fn require_user_presence(plan_id: &str, plan_hash: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "Human approval requires an interactive terminal. Run this command yourself outside agent mode."
        );
    }

    let prompt = format!("Arcula approval required for plan {plan_id} ({plan_hash}). Password: ");

    #[cfg(target_os = "linux")]
    {
        if which::which("pkexec").is_ok() {
            let status = Command::new("pkexec")
                .arg("/bin/true")
                .status()
                .context("Failed to invoke polkit approval prompt via pkexec")?;
            if status.success() {
                return Ok(());
            }
        }
    }

    if which::which("sudo").is_ok() {
        let non_interactive = Command::new("sudo")
            .arg("-k")
            .arg("-n")
            .arg("-v")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("Failed to check sudo approval behavior")?;
        if non_interactive.success() {
            anyhow::bail!(
                "Passwordless sudo cannot be used as human approval. Configure polkit or require a sudo password/Touch ID."
            );
        }

        let status = Command::new("sudo")
            .arg("-k")
            .arg("-v")
            .arg("-p")
            .arg(&prompt)
            .status()
            .context("Failed to invoke sudo approval prompt")?;
        if status.success() {
            return Ok(());
        }
        anyhow::bail!("OS approval was rejected or failed");
    }

    Err(anyhow!(
        "No supported local approval provider found. macOS/Linux require sudo; Linux desktop may use pkexec/polkit."
    ))
}

fn platform_provider() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos_sudo_user_presence"
    }

    #[cfg(target_os = "linux")]
    {
        if which::which("pkexec").is_ok() {
            "linux_polkit"
        } else {
            "linux_sudo_user_presence"
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        "local_user_presence"
    }
}

fn parse_timestamp(value: &str) -> Result<chrono::DateTime<chrono::Utc>> {
    Ok(chrono::DateTime::parse_from_rfc3339(value)?.with_timezone(&chrono::Utc))
}

fn current_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
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
