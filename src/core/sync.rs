use anyhow::{Context, Result};
use colored::Colorize;
use log::error;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

use crate::config::{
    get_environment_kind, is_protected_environment, Environment, EnvironmentKind, MongoConfig,
};
use crate::output;
use crate::utils::mongodb::{self, BackupVerification};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncOptions {
    pub create_backup: bool,
    pub drop_collections: bool,
    pub clear_collections: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            create_backup: true,
            drop_collections: true,
            clear_collections: false,
        }
    }
}

impl SyncOptions {
    pub fn update_collection_settings(&mut self) {
        // If drop is enabled, automatically disable clear as it's redundant
        if self.drop_collections {
            self.clear_collections = false;
        }
    }

    pub fn is_destructive(&self) -> bool {
        self.drop_collections || self.clear_collections
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    pub source_env: Environment,
    pub target_env: Environment,
    pub source_db: String,
    pub target_db: String,
    pub options: SyncOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub source_env: Environment,
    pub target_env: Environment,
    pub source_db: String,
    pub target_db: String,
    pub target_kind: EnvironmentKind,
    pub target_protected: bool,
    pub options: SyncOptions,
    pub backup_path: Option<String>,
    pub backup_verification: Option<BackupVerification>,
    pub restored_from_backup: bool,
}

/// Parse environment string and return Environment enum
pub fn parse_environment(env_str: &str) -> Result<Environment> {
    Environment::from_str(env_str).context(format!("Invalid environment: {}", env_str))
}

/// Get list of databases for a given environment
pub async fn get_databases(env: &Environment) -> Result<Vec<String>> {
    let config = MongoConfig::from_env(env.clone())
        .context(format!("Failed to get configuration for {}", env))?;

    let all_dbs = mongodb::list_databases(&config).await?;

    // Filter out system databases
    let dbs = all_dbs
        .into_iter()
        .filter(|db| !matches!(db.as_str(), "admin" | "local" | "config"))
        .collect();

    Ok(dbs)
}

/// Perform database synchronization with the given configuration
pub async fn perform_sync(mut config: SyncConfig) -> Result<SyncReport> {
    config.options.update_collection_settings();
    mongodb::validate_db_name(&config.source_db)?;
    mongodb::validate_db_name(&config.target_db)?;

    let target_kind = get_environment_kind(&config.target_env);
    let target_protected = is_protected_environment(&config.target_env);
    validate_sync_safety(&config, target_kind, target_protected)?;

    let source_config = MongoConfig::from_env(config.source_env.clone()).context(format!(
        "Failed to get configuration for {}",
        config.source_env
    ))?;

    let target_config = MongoConfig::from_env(config.target_env.clone()).context(format!(
        "Failed to get configuration for {}",
        config.target_env
    ))?;

    if output::is_text() {
        print_sync_plan(&config, target_kind, target_protected);
    }

    perform_sync_single(
        &source_config,
        &target_config,
        &config,
        target_kind,
        target_protected,
    )
    .await
}

fn validate_sync_safety(
    config: &SyncConfig,
    target_kind: EnvironmentKind,
    target_protected: bool,
) -> Result<()> {
    if (target_kind.is_prod() || target_protected)
        && config.options.is_destructive()
        && !config.options.create_backup
    {
        anyhow::bail!(
            "Refusing destructive sync to protected/production target '{}:{}' without a full backup. Set --backup true and ensure backup succeeds before drop/clear.",
            config.target_env,
            config.target_db
        );
    }

    Ok(())
}

fn print_sync_plan(config: &SyncConfig, target_kind: EnvironmentKind, target_protected: bool) {
    println!("\n{}", "Synchronization plan:".bold().underline());
    println!("{} {}", "From:".green().bold(), config.source_env);
    println!("{} {}", "To:".green().bold(), config.target_env);
    println!("{} {}", "Target kind:".green().bold(), target_kind);
    println!(
        "{} {}",
        "Target protected:".green().bold(),
        target_protected
    );
    println!("{} {}", "Source database:".green().bold(), config.source_db);
    println!("{} {}", "Target database:".green().bold(), config.target_db);
    println!(
        "{} {}",
        "Create backup:".green().bold(),
        if config.options.create_backup {
            "Yes"
        } else {
            "No"
        }
    );
    println!(
        "{} {}",
        "Drop collections:".green().bold(),
        if config.options.drop_collections {
            "Yes"
        } else {
            "No"
        }
    );
    println!(
        "{} {}",
        "Clear collections:".green().bold(),
        if config.options.clear_collections {
            "Yes"
        } else {
            "No"
        }
    );
}

/// Perform synchronization between a single source and target database
async fn perform_sync_single(
    source_config: &MongoConfig,
    target_config: &MongoConfig,
    config: &SyncConfig,
    target_kind: EnvironmentKind,
    target_protected: bool,
) -> Result<SyncReport> {
    let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
    let temp_path = temp_dir.path();

    if output::is_text() {
        println!("\nProcessing database: {}", config.source_db);
    }

    let mut backup_verification: Option<BackupVerification> = None;
    let backup_path: Option<PathBuf> = if config.options.create_backup {
        let path = mongodb::create_backup(target_config, &config.target_db)
            .await
            .context("Failed to create required target backup")?;
        let verification = mongodb::verify_backup(&path, &config.target_db)
            .context("Failed to verify target backup")?;

        if (target_kind.is_prod() || target_protected) && !verification.verified {
            anyhow::bail!(
                "Refusing protected/production sync because target backup verification failed: {}",
                verification.backup_path
            );
        }

        if output::is_text() {
            println!("{} {}", "Backup created:".green(), path.display());
            println!(
                "{} verified={}, files={}, bytes={}",
                "Backup verification:".green(),
                verification.verified,
                verification.file_count,
                verification.byte_count
            );
        }

        backup_verification = Some(verification);
        Some(path)
    } else {
        None
    };

    mongodb::export_database(source_config, &config.source_db, temp_path)
        .await
        .context("Failed to export source database")?;

    if output::is_text() {
        println!("{} {}", "Export completed:".green(), config.source_db);
    }

    let export_db_path = temp_path.join(&config.source_db);
    if !export_db_path.exists() {
        error!(
            "Export directory not found at expected path: {}",
            export_db_path.display()
        );
        anyhow::bail!(
            "Export directory not found at: {}. The database may be empty.",
            export_db_path.display()
        );
    }

    if config.source_db != config.target_db {
        let target_db_path = temp_path.join(&config.target_db);
        let _ = std::fs::remove_dir_all(&target_db_path);
        std::fs::rename(&export_db_path, &target_db_path)
            .context("Failed to rename dump directory")?;
        if output::is_text() {
            println!(
                "{} {} -> {}",
                "Renamed export directory:".green(),
                config.source_db,
                config.target_db
            );
        }
    }

    let import_result = mongodb::import_database(
        target_config,
        &config.target_db,
        temp_path,
        config.options.drop_collections,
        config.options.clear_collections,
    )
    .await;

    if let Err(import_err) = import_result {
        error!("Failed to import database: {import_err:#}");

        if let Some(path) = &backup_path {
            if output::is_text() {
                println!("{} {}", "Restoring backup:".yellow(), path.display());
            }

            if let Err(restore_err) =
                mongodb::restore_backup(target_config, &config.target_db, path).await
            {
                error!("Failed to restore backup: {restore_err:#}");
                anyhow::bail!(
                    "Import failed and backup restoration failed. import error: {import_err:#}; restore error: {restore_err:#}"
                );
            }

            anyhow::bail!("Import failed; target backup was restored: {import_err:#}");
        }

        anyhow::bail!("Import failed and no backup was available: {import_err:#}");
    }

    if output::is_text() {
        println!("{} {}", "Import completed:".green(), config.target_db);
        println!("\n{}", "Synchronization completed".green().bold());
    }

    Ok(SyncReport {
        source_env: config.source_env.clone(),
        target_env: config.target_env.clone(),
        source_db: config.source_db.clone(),
        target_db: config.target_db.clone(),
        target_kind,
        target_protected,
        options: config.options.clone(),
        backup_path: backup_path.map(|path| path.display().to_string()),
        backup_verification,
        restored_from_backup: false,
    })
}
