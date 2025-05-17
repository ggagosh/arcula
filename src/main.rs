use anyhow::Result;
use clap::{Parser, Subcommand};
use dotenv::dotenv;
use env_logger::Env;

mod commands;
mod config;
mod core;
mod utils;

#[derive(Parser)]
#[command(name = "mongo-importer")]
#[command(about = "MongoDB database synchronization tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Synchronize data between MongoDB environments
    Sync {
        /// Source environment (LOCAL, DEV, STG, PROD)
        #[arg(short, long)]
        from: Option<String>,

        /// Target environment (LOCAL, DEV, STG, PROD)
        #[arg(short, long)]
        to: Option<String>,

        /// Database to synchronize
        #[arg(short, long)]
        db: Option<String>,

        /// Target database name (defaults to source database name)
        #[arg(short = 'n', long)]
        target_db: Option<String>,

        /// Create backup before import
        #[arg(short, long, default_value = "true")]
        backup: Option<bool>,

        /// Drop collections during import
        #[arg(short = 'D', long, default_value = "true")]
        drop: Option<bool>,

        /// Clear collections during import (ignored if drop is enabled)
        #[arg(short = 'c', long, default_value = "false")]
        clear: Option<bool>,

        /// Interactive mode - prompt for values not provided on command line
        #[arg(short, long)]
        interactive: bool,
    },
    /// Show information about available MongoDB environments
    Info,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize environment
    dotenv().ok();
    env_logger::Builder::from_env(Env::default().default_filter_or("info")).init();

    // Check if MongoDB tools are available
    if let Err(err) = config::check_mongodb_tools() {
        eprintln!("Error: MongoDB tools not found. Please install MongoDB tools (mongodump and mongorestore).");
        eprintln!("Error details: {}", err);

        return Err(anyhow::anyhow!("MongoDB tools not found"));
    }

    // Parse CLI arguments
    let cli = Cli::parse();

    // Process commands
    match cli.command {
        Commands::Sync {
            from,
            to,
            db,
            target_db,
            backup,
            drop,
            clear,
            interactive,
        } => {
            let params = commands::sync::SyncParams {
                from,
                to,
                db,
                target_db,
                backup,
                drop,
                clear,
                interactive,
            };
            commands::sync::execute_with_params(params).await?;
        }
        Commands::Info => {
            commands::info::execute().await?;
        }
    }

    Ok(())
}
