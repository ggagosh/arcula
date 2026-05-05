use anyhow::{anyhow, Result};
use clap::{Args, Subcommand};
use colored::Colorize;
use inquire::{Confirm, MultiSelect, Select, Text};
use serde::Serialize;

use crate::config::{
    get_connection_policy, get_environment_kind, is_protected_environment, Environment,
    EnvironmentKind,
};
use crate::core::sync::{get_databases, parse_environment, perform_sync, SyncConfig, SyncOptions};
use crate::output;
use crate::plans;
use crate::utils::mongodb;

#[derive(Debug, Clone, Default, Args)]
pub struct SyncArgs {
    /// Source environment/connection label
    #[arg(short, long)]
    pub from: Option<String>,

    /// Target environment/connection label
    #[arg(short, long)]
    pub to: Option<String>,

    /// Source MongoDB URI. Bypasses stored connections/.env for the source endpoint.
    #[arg(long, value_name = "URI")]
    pub from_uri: Option<String>,

    /// Target MongoDB URI. Bypasses stored connections/.env for the target endpoint.
    #[arg(long, value_name = "URI")]
    pub to_uri: Option<String>,

    /// Source environment kind override
    #[arg(long, value_enum)]
    pub from_kind: Option<EnvironmentKind>,

    /// Target environment kind override. Use 'prod' for production URIs.
    #[arg(long, value_enum)]
    pub to_kind: Option<EnvironmentKind>,

    /// Database to synchronize
    #[arg(short, long)]
    pub db: Option<String>,

    /// Target database name (defaults to source database name)
    #[arg(short = 'n', long)]
    pub target_db: Option<String>,

    /// Create backup before import
    #[arg(short, long, default_value = "true")]
    pub backup: Option<bool>,

    /// Drop collections during import
    #[arg(short = 'D', long, default_value = "true")]
    pub drop: Option<bool>,

    /// Clear collections during import (ignored if drop is enabled)
    #[arg(short = 'c', long, default_value = "false")]
    pub clear: Option<bool>,

    /// Interactive mode - prompt for values not provided on command line
    #[arg(short, long)]
    pub interactive: bool,

    /// Agent mode: JSON output, no prompts, no colors/progress
    #[arg(long)]
    pub agent: bool,

    /// Dry-run mode - show what would be done without executing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Subcommand)]
pub enum SyncCommand {
    /// Create and save a sync plan without changing any database
    Plan(SyncArgs),

    /// Run sync immediately. Protected targets may require the plan/approval flow.
    Run(SyncArgs),
}

/// Parameters for synchronization operations
pub struct SyncParams {
    pub from: Option<String>,
    pub to: Option<String>,
    pub from_uri: Option<String>,
    pub to_uri: Option<String>,
    pub from_kind: Option<EnvironmentKind>,
    pub to_kind: Option<EnvironmentKind>,
    pub db: Option<String>,
    pub target_db: Option<String>,
    pub backup: Option<bool>,
    pub drop: Option<bool>,
    pub clear: Option<bool>,
    pub interactive: bool,
    pub agent: bool,
    pub dry_run: bool,
}

impl From<SyncArgs> for SyncParams {
    fn from(args: SyncArgs) -> Self {
        Self {
            from: args.from,
            to: args.to,
            from_uri: args.from_uri,
            to_uri: args.to_uri,
            from_kind: args.from_kind,
            to_kind: args.to_kind,
            db: args.db,
            target_db: args.target_db,
            backup: args.backup,
            drop: args.drop,
            clear: args.clear,
            interactive: args.interactive,
            agent: args.agent,
            dry_run: args.dry_run,
        }
    }
}

#[derive(Serialize)]
struct SyncPlan<'a> {
    config: &'a SyncConfig,
    target_kind: EnvironmentKind,
    target_protected: bool,
    destructive: bool,
    requires_full_backup: bool,
    requires_human_approval: bool,
    direct_source_uri: bool,
    direct_target_uri: bool,
}

/// Execute sync with SyncParams struct
pub async fn execute_with_params(params: SyncParams) -> Result<()> {
    configure_agent_output(&params)?;

    if should_use_interactive_mode(&params) {
        execute_interactive(&params).await
    } else {
        execute_non_interactive(&params).await
    }
}

pub fn create_plan_with_args(args: SyncArgs) -> Result<plans::SyncPlanRecord> {
    let params = SyncParams::from(args);
    configure_agent_output(&params)?;
    if params.interactive {
        return Err(anyhow!(
            "sync plan does not support interactive prompts. Pass --from, --to and --db."
        ));
    }
    if params.from_uri.is_some() || params.to_uri.is_some() {
        return Err(anyhow!(
            "Saved plans cannot contain raw direct URIs. Store them first with 'arcula connection add' or use immediate --dry-run."
        ));
    }

    let config = build_non_interactive_config(&params)?;
    let plan = plans::create_and_save_sync_plan(config)?;

    if output::is_json() {
        output::print_json_success("sync_plan", &plan);
    } else {
        println!("{} {}", "Saved sync plan:".green().bold(), plan.id.bold());
        println!();
        println!("{}", plans::render_plan_text(&plan));
        if plan.requires_human_approval {
            println!(
                "{} arcula plan approve {}",
                "Approve with:".yellow().bold(),
                plan.id
            );
        }
        println!(
            "{} arcula operation run {}",
            "Run with:".yellow().bold(),
            plan.id
        );
    }

    Ok(plan)
}

fn should_use_interactive_mode(params: &SyncParams) -> bool {
    params.interactive
        || (!params.agent
            && output::is_text()
            && params.from.is_none()
            && params.to.is_none()
            && params.from_uri.is_none()
            && params.to_uri.is_none()
            && params.db.is_none())
}

fn configure_agent_output(params: &SyncParams) -> Result<()> {
    if params.agent {
        if params.interactive {
            return Err(anyhow!("--agent cannot be combined with --interactive"));
        }
        output::set_output_format(crate::output::OutputFormat::Json);
        colored::control::set_override(false);
    }

    if output::is_json() && params.interactive {
        return Err(anyhow!(
            "Interactive prompts are disabled for JSON output. Pass all required flags."
        ));
    }

    Ok(())
}

async fn execute_interactive(params: &SyncParams) -> Result<()> {
    let source_env = resolve_or_prompt_source_env(params).await?;

    let source_dbs = get_databases(&source_env).await?;
    if source_dbs.is_empty() {
        return Err(anyhow!("No databases found in source environment"));
    }

    let source_db = if let Some(db_str) = params.db.clone() {
        mongodb::validate_db_name(&db_str)?;
        if !source_dbs.contains(&db_str) {
            return Err(anyhow!(
                "Database '{}' not found in source environment",
                db_str
            ));
        }
        db_str
    } else {
        Select::new("2. Select source database:", source_dbs)
            .with_page_size(10)
            .with_help_message("Type to filter databases")
            .prompt()?
    };

    let target_env = resolve_or_prompt_target_env(params).await?;

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

    let target_db_name = if let Some(tgt_db) = &params.target_db {
        mongodb::validate_db_name(tgt_db)?;
        tgt_db.clone()
    } else {
        Text::new("4. Target database:")
            .with_default(&source_db)
            .with_help_message("Press Enter to use the source database name, or type a new one")
            .prompt()?
    };
    mongodb::validate_db_name(&target_db_name)?;

    let mut options = SyncOptions {
        create_backup: params.backup.unwrap_or(true),
        drop_collections: params.drop.unwrap_or(true),
        clear_collections: params.clear.unwrap_or(false),
    };

    let option_labels = vec![
        "Create backup before import",
        "Drop collections during import",
        "Clear collections during import (ignored if drop is enabled)",
    ];

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

    let selected_options = MultiSelect::new("5. Configure sync settings:", option_labels)
        .with_default(&defaults)
        .with_help_message("Space to toggle, Enter to confirm")
        .prompt()?;

    options.create_backup = selected_options.contains(&"Create backup before import");
    options.drop_collections = selected_options.contains(&"Drop collections during import");
    options.clear_collections =
        selected_options.contains(&"Clear collections during import (ignored if drop is enabled)");
    options.update_collection_settings();

    let config = SyncConfig {
        source_env,
        target_env,
        source_db,
        target_db: target_db_name,
        options,
    };
    validate_plan(&config, params)?;

    let operation_pattern = format_operation_pattern(&config);

    let proceed = Confirm::new("6. Ready to proceed with synchronization?")
        .with_default(true)
        .with_help_message(&operation_pattern)
        .prompt()?;

    if !proceed {
        return Ok(());
    }

    if params.dry_run {
        print_dry_run_summary(&config, params)?;
        return Ok(());
    }

    let report = perform_sync(config).await?;
    if output::is_json() {
        output::print_json_success("sync_result", &report);
    }

    Ok(())
}

fn format_operation_pattern(config: &SyncConfig) -> String {
    format!(
        "{}:{} → {}:{}  B:[{}] D:[{}] C:[{}]",
        config.source_env,
        config.source_db,
        config.target_env,
        config.target_db,
        if config.options.create_backup {
            "✓".green()
        } else {
            "✗".yellow()
        },
        if config.options.drop_collections {
            "✓".green()
        } else {
            "✗".yellow()
        },
        if config.options.clear_collections {
            "✓".green()
        } else {
            "✗".yellow()
        }
    )
}

fn print_dry_run_summary(config: &SyncConfig, params: &SyncParams) -> Result<()> {
    let target_kind = get_environment_kind(&config.target_env);
    let target_protected = is_protected_environment(&config.target_env);
    let target_policy = get_connection_policy(&config.target_env);
    let plan = SyncPlan {
        config,
        target_kind,
        target_protected,
        destructive: config.options.is_destructive(),
        requires_full_backup: target_policy.destructive_requires_backup
            && config.options.is_destructive(),
        requires_human_approval: target_policy.human_approval_required,
        direct_source_uri: params.from_uri.is_some(),
        direct_target_uri: params.to_uri.is_some(),
    };

    if output::is_json() {
        output::print_json_success("sync_plan", &plan);
        return Ok(());
    }

    println!("\n{}", "=== DRY RUN MODE ===".yellow().bold());
    println!("The following synchronization would be performed:\n");
    println!(
        "  {} {} → {}",
        "Environments:".green(),
        config.source_env,
        config.target_env
    );
    println!("  {} {}", "Target kind:".green(), target_kind);
    println!("  {} {}", "Target protected:".green(), target_protected);
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
    println!(
        "  {} {}",
        "Requires human approval:".green(),
        target_policy.human_approval_required
    );
    println!("\n{}", "No changes were made.".yellow());

    Ok(())
}

async fn execute_non_interactive(params: &SyncParams) -> Result<()> {
    let config = build_non_interactive_config(params)?;
    validate_plan(&config, params)?;

    if params.dry_run {
        print_dry_run_summary(&config, params)?;
        return Ok(());
    }

    let source_dbs = get_databases(&config.source_env).await?;
    if !source_dbs.contains(&config.source_db) {
        return Err(anyhow!(
            "Database '{}' not found in '{}'. Available: {}",
            config.source_db,
            config.source_env,
            source_dbs.join(", ")
        ));
    }

    let report = perform_sync(config).await?;
    if output::is_json() {
        output::print_json_success("sync_result", &report);
    }

    Ok(())
}

fn build_non_interactive_config(params: &SyncParams) -> Result<SyncConfig> {
    let source_env = resolve_endpoint_env(
        params.from.as_ref(),
        params.from_uri.as_ref(),
        params.from_kind,
        "SOURCE",
        "Source environment is required (--from) unless --from-uri is provided",
    )?;

    let target_env = resolve_endpoint_env(
        params.to.as_ref(),
        params.to_uri.as_ref(),
        params.to_kind,
        "TARGET",
        "Target environment is required (--to) unless --to-uri is provided",
    )?;

    if output::is_text() && source_env == target_env {
        println!(
            "{} Source and target are the same environment ({}). Proceeding anyway.",
            "Warning:".yellow().bold(),
            source_env
        );
    }

    let source_db = match &params.db {
        Some(db_str) => {
            mongodb::validate_db_name(db_str)?;
            db_str.clone()
        }
        None => return Err(anyhow!("Source database is required (--db)")),
    };

    let target_db_name = params
        .target_db
        .clone()
        .unwrap_or_else(|| source_db.clone());
    mongodb::validate_db_name(&target_db_name)?;

    let mut options = SyncOptions {
        create_backup: params.backup.unwrap_or(true),
        drop_collections: params.drop.unwrap_or(true),
        clear_collections: params.clear.unwrap_or(false),
    };
    options.update_collection_settings();

    Ok(SyncConfig {
        source_env,
        target_env,
        source_db,
        target_db: target_db_name,
        options,
    })
}

async fn resolve_or_prompt_source_env(params: &SyncParams) -> Result<Environment> {
    if params.from_uri.is_some() || params.from.is_some() {
        return resolve_endpoint_env(
            params.from.as_ref(),
            params.from_uri.as_ref(),
            params.from_kind,
            "SOURCE",
            "Source environment is required (--from) unless --from-uri is provided",
        );
    }

    let env_options = crate::config::get_available_environments();
    if env_options.is_empty() {
        return Err(anyhow!("No MongoDB environments configured. Use 'info' command to see how to configure environments."));
    }

    let env = Select::new("1. Select source environment:", env_options).prompt()?;
    apply_kind_override(&env, params.from_kind);
    Ok(env)
}

async fn resolve_or_prompt_target_env(params: &SyncParams) -> Result<Environment> {
    if params.to_uri.is_some() || params.to.is_some() {
        return resolve_endpoint_env(
            params.to.as_ref(),
            params.to_uri.as_ref(),
            params.to_kind,
            "TARGET",
            "Target environment is required (--to) unless --to-uri is provided",
        );
    }

    let env_options = crate::config::get_available_environments();
    if env_options.is_empty() {
        return Err(anyhow!("No MongoDB environments configured. Use 'info' command to see how to configure environments."));
    }

    let env = Select::new("3. Select target environment:", env_options).prompt()?;
    apply_kind_override(&env, params.to_kind);
    Ok(env)
}

fn resolve_endpoint_env(
    label: Option<&String>,
    uri: Option<&String>,
    kind: Option<EnvironmentKind>,
    fallback_label: &str,
    missing_message: &str,
) -> Result<Environment> {
    let env = match (label, uri) {
        (Some(label), _) => parse_environment(label)?,
        (None, Some(_)) => parse_environment(fallback_label)?,
        (None, None) => return Err(anyhow!(missing_message.to_string())),
    };

    if let Some(uri) = uri {
        std::env::set_var(format!("MONGO_{}_URI", env.name()), uri);

        if fallback_label == "TARGET" && kind.is_none() {
            std::env::set_var(format!("MONGO_{}_PROTECTED", env.name()), "true");
        }
    }

    apply_kind_override(&env, kind);

    Ok(env)
}

fn apply_kind_override(env: &Environment, kind: Option<EnvironmentKind>) {
    if let Some(kind) = kind {
        std::env::set_var(format!("MONGO_{}_KIND", env.name()), kind.to_string());
    }
}

fn validate_plan(config: &SyncConfig, params: &SyncParams) -> Result<()> {
    let target_kind = get_environment_kind(&config.target_env);
    let target_protected = is_protected_environment(&config.target_env);
    let target_policy = get_connection_policy(&config.target_env);

    if target_policy.destructive_requires_backup
        && config.options.is_destructive()
        && !config.options.create_backup
    {
        return Err(anyhow!(
            "Refusing destructive sync to protected/production target '{}:{}' without a full backup. Set --backup true.",
            config.target_env,
            config.target_db
        ));
    }

    if !params.dry_run && target_policy.human_approval_required {
        return Err(anyhow!(
            "Target '{}:{}' requires human-approved plan execution. Use: arcula sync plan --from {} --to {} --db {} && arcula plan approve <plan-id> && arcula operation run <plan-id>",
            config.target_env,
            config.target_db,
            config.source_env,
            config.target_env,
            config.source_db
        ));
    }

    if (target_kind.is_prod() || target_protected)
        && config.options.is_destructive()
        && !config.options.create_backup
    {
        return Err(anyhow!(
            "Refusing destructive sync to protected/production target '{}:{}' without a full backup. Set --backup true.",
            config.target_env,
            config.target_db
        ));
    }

    Ok(())
}
