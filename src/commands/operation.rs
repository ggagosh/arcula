use anyhow::{anyhow, Result};
use clap::Subcommand;
use colored::Colorize;
use inquire::Confirm;

use crate::operations;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum OperationCommand {
    /// Run an approved/safe sync plan and save an operation record
    Run {
        /// Plan id
        plan_id: String,

        /// Agent execution mode; refuses protected plans without human approval
        #[arg(long)]
        agent: bool,
    },

    /// List saved operations
    List,

    /// Show a saved operation
    Show {
        /// Operation id
        id: String,
    },

    /// Revert an operation by restoring its pre-sync backup
    Revert {
        /// Operation id to revert
        id: String,

        /// Show what would be restored without executing
        #[arg(long)]
        dry_run: bool,

        /// Required non-interactive confirmation token. Use the operation id.
        #[arg(long)]
        confirm: Option<String>,
    },
}

pub async fn execute(command: OperationCommand) -> Result<()> {
    match command {
        OperationCommand::Run { plan_id, agent } => run_operation(&plan_id, agent).await,
        OperationCommand::List => list_operations(),
        OperationCommand::Show { id } => show_operation(&id),
        OperationCommand::Revert {
            id,
            dry_run,
            confirm,
        } => revert_operation(&id, dry_run, confirm).await,
    }
}

async fn run_operation(plan_id: &str, agent: bool) -> Result<()> {
    crate::config::check_mongodb_tools()
        .map_err(|err| anyhow!("MongoDB tools not found: {err}"))?;
    let operation = operations::run_plan(plan_id, agent).await?;

    if output::is_json() {
        output::print_json_success("operation", &operation);
        return Ok(());
    }

    println!(
        "{} {}",
        "Operation completed:".green().bold(),
        operation.id.bold()
    );
    Ok(())
}

fn list_operations() -> Result<()> {
    let operations = operations::list_operations()?;

    if output::is_json() {
        output::print_json_success("operations", &operations);
        return Ok(());
    }

    if operations.is_empty() {
        println!("{}", "No saved operations.".yellow());
        return Ok(());
    }

    println!("\n{}", "Saved Operations:".bold().underline());
    for operation in operations {
        println!(
            "{}  {:?}  {:?}  plan={}",
            operation.id,
            operation.kind,
            operation.status,
            operation.plan_id.as_deref().unwrap_or("-")
        );
    }
    println!();
    Ok(())
}

fn show_operation(id: &str) -> Result<()> {
    let operation = operations::load_operation(id)?;

    if output::is_json() {
        output::print_json_success("operation", &operation);
        return Ok(());
    }

    println!("\n{} {}", "Operation:".green().bold(), operation.id.bold());
    println!("Kind: {:?}", operation.kind);
    println!("Status: {:?}", operation.status);
    if let Some(plan_id) = &operation.plan_id {
        println!("Plan: {plan_id}");
    }
    if let Some(report) = &operation.sync_report {
        println!(
            "Sync: {}:{} -> {}:{}",
            report.source_env, report.source_db, report.target_env, report.target_db
        );
        println!(
            "Backup: {}",
            report.backup_path.as_deref().unwrap_or("none")
        );
    }
    if let Some(error) = &operation.error {
        println!("{} {error}", "Error:".red().bold());
    }
    println!();
    Ok(())
}

async fn revert_operation(id: &str, dry_run: bool, confirm: Option<String>) -> Result<()> {
    let preview = operations::preview_revert(id)?;

    if dry_run {
        if output::is_json() {
            output::print_json_success("revert_preview", &preview);
            return Ok(());
        }
        println!("\n{}", "Revert preview:".yellow().bold());
        println!("Operation: {}", preview.operation_id);
        println!("Target: {}:{}", preview.target_env, preview.target_db);
        println!("Backup: {}", preview.backup_path);
        println!(
            "Human approval required: {}",
            preview.target_policy_requires_human_approval
        );
        return Ok(());
    }

    match confirm {
        Some(token) if token == id => {}
        Some(_) => return Err(anyhow!("Invalid confirmation token. Use --confirm {id}")),
        None if output::is_json() => {
            return Err(anyhow!(
                "Revert requires --confirm {id} in JSON/non-interactive output"
            ));
        }
        None => {
            println!(
                "\n{}",
                "Revert will restore the target database from backup."
                    .yellow()
                    .bold()
            );
            println!("Target: {}:{}", preview.target_env, preview.target_db);
            println!("Backup: {}", preview.backup_path);
            let proceed = Confirm::new(&format!("Type yes to revert operation {id}?"))
                .with_default(false)
                .prompt()?;
            if !proceed {
                println!("Operation cancelled.");
                return Ok(());
            }
        }
    }

    let operation = operations::revert_operation(id).await?;

    if output::is_json() {
        output::print_json_success("operation", &operation);
        return Ok(());
    }

    println!(
        "{} {}",
        "Revert completed:".green().bold(),
        operation.id.bold()
    );
    Ok(())
}
