use anyhow::Result;
use colored::Colorize;
use log::info;
use serde::Serialize;

use crate::config::{
    get_connection_policy, get_environment_kind, is_protected_environment, EnvironmentKind,
    MongoConfig,
};
use crate::connections::ConnectionPolicy;
use crate::output;
use crate::utils::mongodb::{self, mask_connection_string};

#[derive(Serialize)]
struct EnvironmentInfo {
    name: String,
    kind: EnvironmentKind,
    protected: bool,
    policy: ConnectionPolicy,
    connection: Option<String>,
    databases: Vec<String>,
    error: Option<String>,
}

pub async fn execute() -> Result<()> {
    info!("Displaying MongoDB environment information");

    let environments = crate::config::get_available_environments();
    let mut infos = Vec::new();

    if output::is_text() {
        println!("\n{}", "MongoDB Environments:".bold().underline());
    }

    if environments.is_empty() {
        if output::is_json() {
            output::print_json_success("environments", &infos);
            return Ok(());
        }

        println!("\n{}", "No MongoDB connections configured.".yellow());
        println!("Recommended: store URIs securely with the connection manager:");
        println!("  arcula connection add dev --kind dev");
        println!("  arcula connection add prod --kind prod");
        println!("Legacy/CI: configure environment variables like:");
        println!("  MONGO_LOCAL_URI=mongodb://localhost:27017");
        println!("Optional metadata:");
        println!("  MONGO_LOCAL_KIND=local  # local, dev, staging, prod, other");
        return Ok(());
    }

    for env in environments {
        let kind = get_environment_kind(&env);
        let protected = is_protected_environment(&env);
        let policy = get_connection_policy(&env);

        match MongoConfig::from_env(env.clone()) {
            Ok(config) => {
                let connection = mask_connection_string(&config.connection_string);
                let databases_result = mongodb::list_databases(&config).await;

                if output::is_text() {
                    println!(
                        "\n{} {}",
                        "Environment:".green().bold(),
                        env.to_string().bold()
                    );
                    println!("{} {}", "Kind:".yellow(), kind);
                    println!("{} {}", "Protected:".yellow(), protected);
                    println!("{} {}", "Connection:".yellow(), connection);
                }

                match databases_result {
                    Ok(databases) => {
                        let filtered_databases: Vec<String> = databases
                            .into_iter()
                            .filter(|db| !should_skip_db(db))
                            .collect();

                        if output::is_text() {
                            println!("{} {}", "Databases:".yellow(), filtered_databases.len());
                            for db in &filtered_databases {
                                println!("  - {db}");
                            }
                        }

                        infos.push(EnvironmentInfo {
                            name: env.name().to_string(),
                            kind,
                            protected,
                            policy: policy.clone(),
                            connection: Some(connection),
                            databases: filtered_databases,
                            error: None,
                        });
                    }
                    Err(e) => {
                        if output::is_text() {
                            println!("{} Could not list databases: {}", "Error:".red().bold(), e);
                        }

                        infos.push(EnvironmentInfo {
                            name: env.name().to_string(),
                            kind,
                            protected,
                            policy: policy.clone(),
                            connection: Some(connection),
                            databases: Vec::new(),
                            error: Some(e.to_string()),
                        });
                    }
                }
            }
            Err(e) => {
                if output::is_text() {
                    println!(
                        "\n{} {}",
                        "Environment:".green().bold(),
                        env.to_string().bold()
                    );
                    println!("{} {}", "Kind:".yellow(), kind);
                    println!("{} {}", "Protected:".yellow(), protected);
                    println!("{} {}", "Status:".yellow(), "Not configured".red());
                }

                infos.push(EnvironmentInfo {
                    name: env.name().to_string(),
                    kind,
                    protected,
                    policy,
                    connection: None,
                    databases: Vec::new(),
                    error: Some(e.to_string()),
                });
            }
        }
    }

    if output::is_json() {
        output::print_json_success("environments", &infos);
        return Ok(());
    }

    println!(
        "\n{}",
        "To configure additional connections securely:".italic()
    );
    println!("  arcula connection add dev --kind dev");
    println!("  arcula connection add prod --kind prod");
    println!("Legacy/CI env format: MONGO_<ENV>_URI=mongodb://...");
    println!();

    Ok(())
}

fn should_skip_db(db_name: &str) -> bool {
    matches!(db_name, "admin" | "local" | "config")
}
