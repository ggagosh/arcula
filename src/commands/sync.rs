use anyhow::{anyhow, Result};
use colored::Colorize;
use inquire::{Confirm, MultiSelect, Select};

use crate::core::sync::{get_databases, parse_environment, perform_sync, SyncConfig, SyncOptions};

/// Parameters for synchronization operations
pub struct SyncParams {
    pub from: Option<String>,
    pub to: Option<String>,
    pub db: Option<String>,
    pub target_db: Option<String>,
    pub backup: Option<bool>,
    pub drop: Option<bool>,
    pub clear: Option<bool>,
    pub interactive: bool,
    pub dry_run: bool,
}

/// Execute sync with individual parameters (deprecated, use execute_with_params instead)
#[deprecated(since = "0.1.0", note = "use execute_with_params instead")]
#[allow(dead_code, clippy::too_many_arguments)]
pub async fn execute(
    from: Option<String>,
    to: Option<String>,
    db: Option<String>,
    target_db: Option<String>,
    backup: Option<bool>,
    drop: Option<bool>,
    clear: Option<bool>,
    interactive: bool,
) -> Result<()> {
    let params = SyncParams {
        from,
        to,
        db,
        target_db,
        backup,
        drop,
        clear,
        interactive,
        dry_run: false,
    };

    execute_with_params(params).await
}

/// Execute sync with SyncParams struct
pub async fn execute_with_params(params: SyncParams) -> Result<()> {
    if params.interactive {
        execute_interactive(&params).await
    } else {
        execute_non_interactive(&params).await
    }
}

async fn execute_interactive(params: &SyncParams) -> Result<()> {
    // Clean, streamlined UI - no introductory messages

    // Step 1: Select source environment
    let source_env = if let Some(from_str) = &params.from {
        parse_environment(from_str)?
    } else {
        // Dynamically get all available environments
        let env_options = crate::config::get_available_environments();

        if env_options.is_empty() {
            return Err(anyhow!("No MongoDB environments configured. Use 'info' command to see how to configure environments."));
        }

        Select::new("1. Select source environment:", env_options).prompt()?
    };

    // Step 2: Select source database with autocomplete
    let source_dbs = get_databases(&source_env).await?;
    if source_dbs.is_empty() {
        return Err(anyhow!("No databases found in source environment"));
    }

    let source_db = if let Some(db_str) = params.db.clone() {
        if !source_dbs.contains(&db_str) {
            return Err(anyhow!(
                "Database '{}' not found in source environment",
                db_str
            ));
        }
        db_str
    } else {
        // Use Select with autocomplete for source database selection
        Select::new("2. Select source database:", source_dbs)
            .with_page_size(10) // Show 10 items at a time
            .with_help_message("Type to filter databases")
            .prompt()?
    };

    // Step 3: Select target environment
    let target_env = if let Some(to_str) = &params.to {
        parse_environment(to_str)?
    } else {
        // Dynamically get all available environments
        let env_options = crate::config::get_available_environments();

        if env_options.is_empty() {
            return Err(anyhow!("No MongoDB environments configured. Use 'info' command to see how to configure environments."));
        }

        Select::new("3. Select target environment:", env_options).prompt()?
    };

    if source_env == target_env {
        println!(
            "{} Source and target are the same environment ({})",
            "Warning:".yellow().bold(),
            source_env
        );
        let proceed = Confirm::new("Are you sure you want to proceed?")
            .with_default(false)
            .prompt()?;
        if !proceed {
            println!("Operation cancelled.");
            return Ok(());
        }
    }

    // Step 4: Select target database with autocomplete
    let target_db_name = if let Some(tgt_db) = &params.target_db {
        tgt_db.clone()
    } else {
        // Fetch available databases from target environment for autocomplete
        let target_dbs = get_databases(&target_env).await?;
        if target_dbs.is_empty() {
            return Err(anyhow!("No databases found in target environment"));
        }

        // If source DB exists in target environment, use it as default selection
        let default_index = target_dbs.iter().position(|db| *db == source_db);

        // Use Select with autocomplete for target database selection
        let select = Select::new("4. Select target database:", target_dbs)
            .with_page_size(10) // Show 10 items at a time
            .with_help_message("Type to filter databases"); // Show help text

        // Set default selection if source DB is in the list
        let select = if let Some(idx) = default_index {
            select.with_starting_cursor(idx)
        } else {
            select
        };

        select.prompt()?
    };

    // Step 5: Configure sync settings
    let mut options = SyncOptions {
        create_backup: params.backup.unwrap_or(true),
        drop_collections: params.drop.unwrap_or(true),
        clear_collections: params.clear.unwrap_or(false),
    };

    // Create option labels
    let option_labels = vec![
        "Create backup before import",
        "Drop collections during import",
        "Clear collections during import (ignored if drop is enabled)",
    ];

    // Set default selections based on initial options
    let mut defaults = Vec::new();
    if options.create_backup {
        defaults.push(0);
    }
    if options.drop_collections {
        defaults.push(1);
    }
    if options.clear_collections {
        defaults.push(2);
    }

    // Show MultiSelect for options
    let selected_options = MultiSelect::new("5. Configure sync settings:", option_labels)
        .with_default(&defaults)
        .with_help_message("Space to toggle, Enter to confirm")
        .prompt()?;

    // Update options based on selections
    options.create_backup = selected_options.contains(&"Create backup before import");
    options.drop_collections = selected_options.contains(&"Drop collections during import");
    options.clear_collections =
        selected_options.contains(&"Clear collections during import (ignored if drop is enabled)");

    // Update settings for consistency
    options.update_collection_settings();

    // Format operation pattern for confirmation
    let operation_pattern = format!(
        "{}:{} → {}:{}  B:[{}] D:[{}] C:[{}]",
        source_env,
        source_db,
        target_env,
        target_db_name,
        if options.create_backup {
            "✓".green()
        } else {
            "✗".yellow()
        },
        if options.drop_collections {
            "✓".green()
        } else {
            "✗".yellow()
        },
        if options.clear_collections {
            "✓".green()
        } else {
            "✗".yellow()
        }
    );

    // Step 6: Confirm and execute sync
    let proceed = Confirm::new("6. Ready to proceed with synchronization?")
        .with_default(true)
        .with_help_message(&operation_pattern)
        .prompt()?;

    if !proceed {
        return Ok(());
    }

    // Create sync config
    let config = SyncConfig {
        source_env,
        target_env,
        source_db,
        target_db: target_db_name,
        options,
    };

    if params.dry_run {
        print_dry_run_summary(&config);
        return Ok(());
    }

    perform_sync(config).await
}

fn print_dry_run_summary(config: &SyncConfig) {
    println!("\n{}", "=== DRY RUN MODE ===".yellow().bold());
    println!("The following synchronization would be performed:\n");
    println!(
        "  {} {} → {}",
        "Environments:".green(),
        config.source_env,
        config.target_env
    );
    println!(
        "  {} {} → {}",
        "Databases:".green(),
        config.source_db,
        config.target_db
    );
    println!(
        "  {} {}",
        "Create backup:".green(),
        if config.options.create_backup {
            "Yes"
        } else {
            "No"
        }
    );
    println!(
        "  {} {}",
        "Drop collections:".green(),
        if config.options.drop_collections {
            "Yes"
        } else {
            "No"
        }
    );
    println!(
        "  {} {}",
        "Clear collections:".green(),
        if config.options.clear_collections {
            "Yes"
        } else {
            "No"
        }
    );
    println!("\n{}", "No changes were made.".yellow());
}

async fn execute_non_interactive(params: &SyncParams) -> Result<()> {
    let source_env = match &params.from {
        Some(env_str) => parse_environment(env_str)?,
        None => return Err(anyhow!("Source environment is required (--from)")),
    };

    let target_env = match &params.to {
        Some(env_str) => parse_environment(env_str)?,
        None => return Err(anyhow!("Target environment is required (--to)")),
    };

    if source_env == target_env {
        println!(
            "{} Source and target are the same environment ({}). Proceeding anyway.",
            "Warning:".yellow().bold(),
            source_env
        );
    }

    let source_db = match &params.db {
        Some(db_str) => db_str.clone(),
        None => return Err(anyhow!("Source database is required (--db)")),
    };

    let source_dbs = get_databases(&source_env).await?;
    if !source_dbs.contains(&source_db) {
        return Err(anyhow!(
            "Database '{}' not found in '{}'. Available: {}",
            source_db,
            source_env,
            source_dbs.join(", ")
        ));
    }

    let target_db_name = params
        .target_db
        .clone()
        .unwrap_or_else(|| source_db.clone());

    let mut options = SyncOptions {
        create_backup: params.backup.unwrap_or(true),
        drop_collections: params.drop.unwrap_or(true),
        clear_collections: params.clear.unwrap_or(false),
    };
    options.update_collection_settings();

    let config = SyncConfig {
        source_env,
        target_env,
        source_db,
        target_db: target_db_name,
        options,
    };

    if params.dry_run {
        print_dry_run_summary(&config);
        return Ok(());
    }

    perform_sync(config).await
}
