use anyhow::{Context, Result};
use colored::Colorize;
use log::error;
use std::path::PathBuf;
use std::str::FromStr;

use crate::config::{Environment, MongoConfig};
use crate::utils::mongodb;

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
}

pub struct SyncConfig {
    pub source_env: Environment,
    pub target_env: Environment,
    pub source_db: String,
    pub target_db: String,
    pub options: SyncOptions,
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
pub async fn perform_sync(config: SyncConfig) -> Result<()> {
    let source_config = MongoConfig::from_env(config.source_env.clone()).context(format!(
        "Failed to get configuration for {}",
        config.source_env
    ))?;

    let target_config = MongoConfig::from_env(config.target_env.clone()).context(format!(
        "Failed to get configuration for {}",
        config.target_env
    ))?;

    // Show summary before execution
    println!("\n{}", "Synchronization plan:".bold().underline());
    println!("{} {}", "From:".green().bold(), config.source_env);
    println!("{} {}", "To:".green().bold(), config.target_env);
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

    perform_sync_single(
        &source_config,
        &target_config,
        &config.source_db,
        &config.target_db,
        config.options.create_backup,
        config.options.drop_collections,
        config.options.clear_collections,
    )
    .await
}

/// Perform synchronization between a single source and target database
async fn perform_sync_single(
    source_config: &MongoConfig,
    target_config: &MongoConfig,
    source_db: &str,
    target_db: &str,
    should_backup: bool,
    drop_collections: bool,
    clear_collections: bool,
) -> Result<()> {
    // Create temporary directory for export/import
    let temp_dir = tempfile::tempdir().context("Failed to create temporary directory")?;
    let temp_path = temp_dir.path();

    println!("\nProcessing database: {}", source_db);

    // Backup target database if requested
    let mut backup_path: Option<PathBuf> = None;
    if should_backup {
        match mongodb::create_backup(target_config, target_db).await {
            Ok(path) => {
                let path_display = path.display().to_string();
                backup_path = Some(path);
                println!("{} {}", "Backup created:".green(), path_display);
            }
            Err(e) => {
                error!("Failed to create backup: {}", e);
                println!(
                    "{} Failed to create backup, proceeding without backup",
                    "Warning:".yellow().bold()
                );
            }
        }
    }

    // Export database from source
    match mongodb::export_database(source_config, source_db, temp_path).await {
        Ok(_) => {
            println!("{} {}", "Export completed:".green(), source_db);

            // Verify the export directory structure
            let export_db_path = temp_path.join(source_db);
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

            if source_db != target_db {
                let target_db_path = temp_path.join(target_db);
                let _ = std::fs::remove_dir_all(&target_db_path);
                std::fs::rename(&export_db_path, &target_db_path)?;
                println!(
                    "{} {} -> {}",
                    "Renamed export directory:".green(),
                    source_db,
                    target_db
                );
            }

            // Import database to target
            match mongodb::import_database(
                target_config,
                target_db,
                temp_path,
                drop_collections,
                clear_collections,
            )
            .await
            {
                Ok(_) => {
                    println!("{} {}", "Import completed:".green(), target_db);
                }
                Err(e) => {
                    error!("Failed to import database: {}", e);
                    println!("{} Import failed: {}", "Error:".red().bold(), e);

                    // Restore backup if available
                    if let Some(path) = &backup_path {
                        println!("{} {}", "Restoring backup:".yellow(), path.display());
                        if let Err(restore_err) =
                            mongodb::restore_backup(target_config, target_db, path).await
                        {
                            error!("Failed to restore backup: {}", restore_err);
                            println!(
                                "{} Backup restoration failed: {}",
                                "Error:".red().bold(),
                                restore_err
                            );
                        } else {
                            println!("{}", "Backup restored successfully".green());
                        }
                    }
                }
            }
        }
        Err(e) => {
            error!("Failed to export database: {}", e);
            println!("{} Export failed: {}", "Error:".red().bold(), e);
        }
    }

    println!("\n{}", "Synchronization completed".green().bold());

    Ok(())
}
