use anyhow::Result;
use clap::Subcommand;
use colored::Colorize;
use serde::Serialize;

use crate::approvals;
use crate::output;
use crate::plans::{self, PlanStatus};

#[derive(Debug, Subcommand)]
pub enum PlanCommand {
    /// List saved sync plans
    List,

    /// Show a saved sync plan
    Show {
        /// Plan id
        id: String,

        /// Render as Markdown in text output
        #[arg(long)]
        markdown: bool,
    },

    /// Approve a saved sync plan. Protected plans require OS user presence.
    Approve {
        /// Plan id
        id: String,
    },
}

#[derive(Serialize)]
struct PlanApprovalResult<T> {
    plan: T,
    approval_saved: bool,
}

pub fn execute(command: PlanCommand) -> Result<()> {
    match command {
        PlanCommand::List => list_plans(),
        PlanCommand::Show { id, markdown } => show_plan(&id, markdown),
        PlanCommand::Approve { id } => approve_plan(&id),
    }
}

fn list_plans() -> Result<()> {
    let plans = plans::list_plans()?;

    if output::is_json() {
        output::print_json_success("plans", &plans);
        return Ok(());
    }

    if plans.is_empty() {
        println!("{}", "No saved sync plans.".yellow());
        return Ok(());
    }

    println!("\n{}", "Saved Sync Plans:".bold().underline());
    for plan in plans {
        println!(
            "{}  {:?}  {}:{} -> {}:{}",
            plan.id,
            plan.status,
            plan.config.source_env,
            plan.config.source_db,
            plan.config.target_env,
            plan.config.target_db
        );
    }
    println!();
    Ok(())
}

fn show_plan(id: &str, markdown: bool) -> Result<()> {
    let plan = plans::load_plan(id)?;

    if output::is_json() {
        output::print_json_success("plan", &plan);
        return Ok(());
    }

    if markdown {
        println!("{}", plans::render_plan_markdown(&plan));
    } else {
        println!("{}", plans::render_plan_text(&plan));
    }
    Ok(())
}

fn approve_plan(id: &str) -> Result<()> {
    let plan = plans::load_plan(id)?;
    let approval = if plan.requires_human_approval {
        approvals::create_human_presence_approval(&plan.id, &plan.hash)?
    } else {
        approvals::create_agent_policy_approval(&plan.id, &plan.hash)?
    };
    approvals::verify_approval(&approval, &plan.id, &plan.hash)?;
    plans::save_approval(&plan.id, &approval)?;
    let plan = plans::update_plan_status(&plan.id, PlanStatus::Approved)?;

    if output::is_json() {
        let result = PlanApprovalResult {
            plan,
            approval_saved: true,
        };
        output::print_json_success("plan_approval", &result);
        return Ok(());
    }

    println!(
        "{} {} ({})",
        "Approved plan:".green().bold(),
        id.bold(),
        approval.provider
    );
    println!("Approval expires at: {}", approval.expires_at);
    Ok(())
}
