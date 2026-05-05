use std::io::{self, Read};

use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use colored::Colorize;
use inquire::{Confirm, Password, PasswordDisplayMode};
use serde::Serialize;

use crate::config::{Environment, EnvironmentKind, MongoConfig};
use crate::connections::{self, ConnectionInfo, ConnectionPolicyPatch};
use crate::output;
use crate::utils::mongodb;

#[derive(Debug, Subcommand)]
pub enum ConnectionCommand {
    /// Add or update a named MongoDB connection in secure storage
    Add {
        /// Connection name used by --from/--to, e.g. dev or prod
        name: String,

        /// Environment kind for safety rules
        #[arg(long, value_enum, default_value_t = EnvironmentKind::Other)]
        kind: EnvironmentKind,

        /// Mark this connection as protected even if it is not prod
        #[arg(long)]
        protected: bool,

        /// MongoDB URI. Prefer prompt or --uri-stdin to avoid shell history.
        #[arg(long, value_name = "URI", conflicts_with = "uri_stdin")]
        uri: Option<String>,

        /// Read the MongoDB URI from stdin
        #[arg(long, conflicts_with = "uri")]
        uri_stdin: bool,

        /// Update an existing connection
        #[arg(long)]
        force: bool,
    },

    /// List configured connections without revealing raw URIs
    List {
        /// Include masked URI values; may access secure storage for each connection
        #[arg(long)]
        show_uri: bool,
    },

    /// Show one connection without revealing the raw URI
    Show {
        /// Connection name
        name: String,
    },

    /// Test that a connection can reach MongoDB
    Test {
        /// Connection name
        name: String,
    },

    /// Update per-connection safety policy flags
    Policy {
        /// Connection name
        name: String,

        /// Whether this connection may be used as sync source
        #[arg(long)]
        allow_as_source: Option<bool>,

        /// Whether this connection may be used as sync target
        #[arg(long)]
        allow_as_target: Option<bool>,

        /// Whether agents may execute plans targeting this connection without human approval
        #[arg(long)]
        allow_agent_apply: Option<bool>,

        /// Whether sync plans targeting this connection require OS-backed human approval
        #[arg(long)]
        human_approval_required: Option<bool>,

        /// Whether destructive syncs targeting this connection require a backup
        #[arg(long)]
        destructive_requires_backup: Option<bool>,

        /// Whether target backups must be verified before destructive sync continues
        #[arg(long)]
        backup_verification_required: Option<bool>,
    },

    /// Remove a connection from metadata and secure storage
    Remove {
        /// Connection name
        name: String,

        /// Do not prompt for confirmation
        #[arg(long)]
        yes: bool,
    },

    /// Import MONGO_<ENV>_URI entries from the loaded environment into secure storage
    ImportEnv {
        /// Update existing stored connections
        #[arg(long)]
        force: bool,
    },
}

#[derive(Serialize)]
struct ConnectionTestResult {
    connection: ConnectionInfo,
    database_count: usize,
    databases: Vec<String>,
}

pub async fn execute(command: ConnectionCommand) -> Result<()> {
    match command {
        ConnectionCommand::Add {
            name,
            kind,
            protected,
            uri,
            uri_stdin,
            force,
        } => add_connection(&name, kind, protected, uri, uri_stdin, force),
        ConnectionCommand::List { show_uri } => list_connections(show_uri),
        ConnectionCommand::Show { name } => show_connection(&name),
        ConnectionCommand::Test { name } => test_connection(&name).await,
        ConnectionCommand::Policy {
            name,
            allow_as_source,
            allow_as_target,
            allow_agent_apply,
            human_approval_required,
            destructive_requires_backup,
            backup_verification_required,
        } => update_policy(
            &name,
            ConnectionPolicyPatch {
                allow_as_source,
                allow_as_target,
                allow_agent_apply,
                human_approval_required,
                destructive_requires_backup,
                backup_verification_required,
            },
        ),
        ConnectionCommand::Remove { name, yes } => remove_connection(&name, yes),
        ConnectionCommand::ImportEnv { force } => import_env_connections(force),
    }
}

fn add_connection(
    name: &str,
    kind: EnvironmentKind,
    protected: bool,
    uri: Option<String>,
    uri_stdin: bool,
    force: bool,
) -> Result<()> {
    let uri = read_uri(uri, uri_stdin)?;
    let info = connections::upsert_connection(name, uri.trim(), kind, protected, force)?;

    if output::is_json() {
        output::print_json_success("connection", &info);
        return Ok(());
    }

    println!(
        "{} {} ({}, protected: {})",
        "Saved connection:".green().bold(),
        info.name.bold(),
        info.kind,
        info.protected
    );
    if let Some(uri_masked) = info.uri_masked {
        println!("{} {}", "URI:".yellow(), uri_masked);
    }
    println!(
        "{} {}",
        "Metadata:".yellow(),
        connections::config_path().display()
    );

    Ok(())
}

fn list_connections(show_uri: bool) -> Result<()> {
    let infos = connections::list_infos(show_uri)?;

    if output::is_json() {
        output::print_json_success("connections", &infos);
        return Ok(());
    }

    if infos.is_empty() {
        println!("{}", "No stored connections configured.".yellow());
        println!("Add one with: arcula connection add dev --kind dev");
        return Ok(());
    }

    println!("\n{}", "Stored MongoDB Connections:".bold().underline());
    for info in infos {
        println!("\n{} {}", "Connection:".green().bold(), info.name.bold());
        println!("{} {}", "Kind:".yellow(), info.kind);
        println!("{} {}", "Protected:".yellow(), info.protected);
        println!("{} {}", "Secret ref:".yellow(), info.secret_ref);
        println!(
            "{} source={}, target={}, agent_apply={}, human_approval={}, destructive_backup={}, backup_verify={}",
            "Policy:".yellow(),
            info.policy.allow_as_source,
            info.policy.allow_as_target,
            info.policy.allow_agent_apply,
            info.policy.human_approval_required,
            info.policy.destructive_requires_backup,
            info.policy.backup_verification_required
        );
        if let Some(uri_masked) = info.uri_masked {
            println!("{} {}", "URI:".yellow(), uri_masked);
        }
    }
    println!();

    Ok(())
}

fn show_connection(name: &str) -> Result<()> {
    let Some(info) = connections::get_info(name, true)? else {
        return Err(anyhow!("Connection '{}' does not exist", name));
    };

    if output::is_json() {
        output::print_json_success("connection", &info);
        return Ok(());
    }

    println!("\n{} {}", "Connection:".green().bold(), info.name.bold());
    println!("{} {}", "Kind:".yellow(), info.kind);
    println!("{} {}", "Protected:".yellow(), info.protected);
    println!("{} {}", "Secret ref:".yellow(), info.secret_ref);
    println!(
        "{} source={}, target={}, agent_apply={}, human_approval={}, destructive_backup={}, backup_verify={}",
        "Policy:".yellow(),
        info.policy.allow_as_source,
        info.policy.allow_as_target,
        info.policy.allow_agent_apply,
        info.policy.human_approval_required,
        info.policy.destructive_requires_backup,
        info.policy.backup_verification_required
    );
    if let Some(uri_masked) = info.uri_masked {
        println!("{} {}", "URI:".yellow(), uri_masked);
    }
    println!();

    Ok(())
}

fn update_policy(name: &str, patch: ConnectionPolicyPatch) -> Result<()> {
    let info = connections::update_policy(name, patch)?;

    if output::is_json() {
        output::print_json_success("connection", &info);
        return Ok(());
    }

    println!(
        "{} {}",
        "Updated connection policy:".green().bold(),
        info.name.bold()
    );
    println!(
        "{} source={}, target={}, agent_apply={}, human_approval={}, destructive_backup={}, backup_verify={}",
        "Policy:".yellow(),
        info.policy.allow_as_source,
        info.policy.allow_as_target,
        info.policy.allow_agent_apply,
        info.policy.human_approval_required,
        info.policy.destructive_requires_backup,
        info.policy.backup_verification_required
    );
    if info.protected || info.kind.is_prod() {
        println!(
            "{} Protected/prod safety floor forces agent_apply=false, human_approval=true, destructive_backup=true, backup_verify=true.",
            "Note:".yellow().bold()
        );
    }
    Ok(())
}

async fn test_connection(name: &str) -> Result<()> {
    let normalized = connections::normalize_name(name)?;
    let Some(uri) = connections::get_uri(&normalized)? else {
        return Err(anyhow!("Connection '{}' does not exist", normalized));
    };
    let Some(metadata) = connections::find_metadata(&normalized)? else {
        return Err(anyhow!("Connection '{}' does not exist", normalized));
    };

    let config = MongoConfig {
        connection_string: uri,
        environment: Environment::new(&normalized),
    };
    let databases = mongodb::list_databases(&config).await?;
    let filtered_databases: Vec<String> = databases
        .into_iter()
        .filter(|db| !matches!(db.as_str(), "admin" | "local" | "config"))
        .collect();
    let info = ConnectionInfo {
        name: metadata.name,
        kind: metadata.kind,
        protected: metadata.protected,
        secret_ref: metadata.secret_ref,
        policy: metadata.policy,
        uri_masked: Some(mongodb::mask_connection_string(&config.connection_string)),
    };
    let result = ConnectionTestResult {
        connection: info,
        database_count: filtered_databases.len(),
        databases: filtered_databases,
    };

    if output::is_json() {
        output::print_json_success("connection_test", &result);
        return Ok(());
    }

    println!(
        "{} {} reachable ({} user databases)",
        "Connection test passed:".green().bold(),
        result.connection.name.bold(),
        result.database_count
    );
    for db in &result.databases {
        println!("  - {db}");
    }

    Ok(())
}

fn remove_connection(name: &str, yes: bool) -> Result<()> {
    if !yes {
        if output::is_json() {
            return Err(anyhow!(
                "Refusing to prompt in JSON output. Pass --yes to remove the connection."
            ));
        }

        let normalized = connections::normalize_name(name)?;
        let proceed = Confirm::new(&format!("Remove stored connection '{}' ?", normalized))
            .with_default(false)
            .prompt()?;
        if !proceed {
            println!("Operation cancelled.");
            return Ok(());
        }
    }

    let info = connections::remove_connection(name)?;

    if output::is_json() {
        output::print_json_success("connection_removed", &info);
        return Ok(());
    }

    println!(
        "{} {}",
        "Removed connection:".green().bold(),
        info.name.bold()
    );
    Ok(())
}

fn import_env_connections(force: bool) -> Result<()> {
    let infos = connections::import_env_connections(force)?;

    if output::is_json() {
        output::print_json_success("connections_imported", &infos);
        return Ok(());
    }

    if infos.is_empty() {
        println!(
            "{}",
            "No new MONGO_<ENV>_URI entries were imported.".yellow()
        );
        if !force {
            println!("Existing stored connections were skipped. Pass --force to update them.");
        }
        return Ok(());
    }

    println!(
        "{} {} connection(s)",
        "Imported".green().bold(),
        infos.len()
    );
    for info in infos {
        println!(
            "  - {} ({}, protected: {})",
            info.name, info.kind, info.protected
        );
    }

    Ok(())
}

fn read_uri(uri: Option<String>, uri_stdin: bool) -> Result<String> {
    if let Some(uri) = uri {
        return Ok(uri);
    }

    if uri_stdin {
        let mut buffer = String::new();
        io::stdin()
            .read_to_string(&mut buffer)
            .context("Failed to read MongoDB URI from stdin")?;
        return Ok(buffer.trim().to_string());
    }

    if output::is_json() {
        return Err(anyhow!(
            "MongoDB URI is required in JSON output. Use --uri or --uri-stdin."
        ));
    }

    let uri = Password::new("MongoDB URI:")
        .without_confirmation()
        .with_display_mode(PasswordDisplayMode::Hidden)
        .with_display_toggle_enabled()
        .prompt()?;

    Ok(uri)
}
