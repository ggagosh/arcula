use anyhow::Result;
use colored::Colorize;
use log::info;

use crate::config::MongoConfig;
use crate::utils::mongodb::{self, mask_connection_string};

pub async fn execute() -> Result<()> {
    info!("Displaying MongoDB environment information");

    // Dynamically get all available environments from environment variables
    let environments = crate::config::get_available_environments();

    println!("\n{}", "MongoDB Environments:".bold().underline());

    if environments.is_empty() {
        println!("\n{}", "No MongoDB environments configured.".yellow());
        println!("Configure environments by setting environment variables like:");
        println!("  MONGO_LOCAL_URI=mongodb://localhost:27017");
        println!("  MONGO_DEV_URI=mongodb://user:password@dev.example.com:27017");
        println!("  MONGO_GIO_URI=mongodb://user:password@gio.example.com:27017");
        return Ok(());
    }

    for env in environments {
        match MongoConfig::from_env(env.clone()) {
            Ok(config) => {
                println!(
                    "\n{} {}",
                    "Environment:".green().bold(),
                    env.to_string().bold()
                );
                println!(
                    "{} {}",
                    "Connection:".yellow(),
                    mask_connection_string(&config.connection_string)
                );

                match mongodb::list_databases(&config).await {
                    Ok(databases) => {
                        println!("{} {}", "Databases:".yellow(), databases.len());
                        for db in databases {
                            if !should_skip_db(&db) {
                                println!("  - {}", db);
                            }
                        }
                    }
                    Err(e) => {
                        println!("{} Could not list databases: {}", "Error:".red().bold(), e);
                    }
                }
            }
            Err(_) => {
                println!(
                    "\n{} {}",
                    "Environment:".green().bold(),
                    env.to_string().bold()
                );
                println!("{} {}", "Status:".yellow(), "Not configured".red());
            }
        }
    }

    println!(
        "\n{}",
        "To configure additional environments, set environment variables in the format:".italic()
    );
    println!("  MONGO_<ENV>_URI=mongodb://...  (e.g. MONGO_GIO_URI)");
    println!();

    Ok(())
}

fn should_skip_db(db_name: &str) -> bool {
    // Skip system databases
    matches!(db_name, "admin" | "local" | "config")
}
