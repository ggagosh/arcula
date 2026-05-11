use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use env_logger::Env;

use crate::output::OutputFormat;

mod approvals;
mod commands;
mod config;
mod connections;
mod core;
mod operations;
mod output;
mod plans;
mod storage;
mod utils;

#[derive(Parser)]
#[command(name = "arcula")]
#[command(about = "Arcula - MongoDB database synchronization tool", long_about = None)]
struct Cli {
    /// Output format for humans or agents
    #[arg(long, value_enum, default_value_t = OutputFormat::Text, global = true)]
    format: OutputFormat,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    /// Load .env from the current directory for legacy/CI workflows
    #[arg(long, global = true)]
    env: bool,

    /// Deprecated: .env loading is disabled by default
    #[arg(long, global = true, hide = true, conflicts_with = "env")]
    no_env: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Synchronize data between MongoDB environments
    Sync {
        #[command(subcommand)]
        command: Option<commands::sync::SyncCommand>,

        #[command(flatten)]
        args: commands::sync::SyncArgs,
    },

    /// Manage saved sync plans and human approvals
    Plan {
        #[command(subcommand)]
        command: commands::plan::PlanCommand,
    },

    /// Run, inspect, and revert saved operations
    Operation {
        #[command(subcommand)]
        command: commands::operation::OperationCommand,
    },

    /// Show information about available MongoDB environments and stored connections
    Info,

    /// Manage named MongoDB connections in secure storage
    Connection {
        #[command(subcommand)]
        command: commands::connection::ConnectionCommand,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let output_format = if command_forces_json(&cli.command) {
        OutputFormat::Json
    } else {
        cli.format
    };
    output::set_output_format(output_format);

    if cli.no_color || output_format.is_json() {
        colored::control::set_override(false);
    }

    if cli.env && !cli.no_env {
        if let Err(e) = dotenv() {
            if output::is_text() && std::path::Path::new(".env").exists() {
                eprintln!("Warning: Failed to parse .env file: {e}");
            }
        }
    }

    let default_log_level = if output_format.is_json() {
        "off"
    } else {
        "info"
    };
    env_logger::Builder::from_env(Env::default().default_filter_or(default_log_level)).init();

    if let Err(err) = run(cli).await {
        if output::is_json() {
            output::print_json_error("operation_failed", &err);
            std::process::exit(1);
        }
        return Err(err);
    }

    Ok(())
}

fn command_forces_json(command: &Commands) -> bool {
    match command {
        Commands::Sync { command, args } => {
            args.agent
                || matches!(command, Some(commands::sync::SyncCommand::Plan(plan_args) | commands::sync::SyncCommand::Run(plan_args)) if plan_args.agent)
        }
        Commands::Operation { command } => {
            matches!(
                command,
                commands::operation::OperationCommand::Run { agent: true, .. }
            )
        }
        _ => false,
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Sync { command, args } => match command {
            Some(commands::sync::SyncCommand::Plan(plan_args)) => {
                commands::sync::create_plan_with_args(plan_args)?;
            }
            Some(commands::sync::SyncCommand::Run(run_args)) => {
                run_immediate_sync(run_args).await?;
            }
            None => {
                run_immediate_sync(args).await?;
            }
        },
        Commands::Plan { command } => {
            commands::plan::execute(command)?;
        }
        Commands::Operation { command } => {
            commands::operation::execute(command).await?;
        }
        Commands::Info => {
            commands::info::execute().await?;
        }
        Commands::Connection { command } => {
            commands::connection::execute(command).await?;
        }
    }

    Ok(())
}

async fn run_immediate_sync(args: commands::sync::SyncArgs) -> Result<()> {
    if !args.dry_run {
        config::check_mongodb_tools().map_err(|err| anyhow!("MongoDB tools not found: {err}"))?;
    }
    commands::sync::execute_with_params(args.into()).await
}
