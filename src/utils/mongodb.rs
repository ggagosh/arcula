use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use log::{error, info};
use std::path::Path;
use std::process::{Command, Stdio};
use std::str;

use crate::config::{get_backup_dir, get_mongodb_bin_path, MongoConfig};

pub async fn list_databases(config: &MongoConfig) -> Result<Vec<String>> {
    let client_options = config.get_client_options().await?;
    let client = mongodb::Client::with_options(client_options)?;

    let db_names = client.list_database_names().await?;

    Ok(db_names)
}

pub async fn export_database(
    config: &MongoConfig,
    database: &str,
    output_dir: &Path,
) -> Result<()> {
    info!(
        "Exporting database {} from {}",
        database, config.environment
    );

    let progress = create_progress_bar("Exporting");

    let bin_path = get_mongodb_bin_path().map_err(|e| {
        error!("Failed to find MongoDB tools: {}", e);
        anyhow::anyhow!("Failed to find mongodump")
    })?;
    let mongodump_path = bin_path.join("mongodump");

    info!("Using mongodump from: {}", mongodump_path.display());
    info!("MongoDB connection string: {}", config.connection_string);

    // Use the traditional --db flag for mongodump (compatible with older versions)
    let output = Command::new(mongodump_path)
        .arg("--uri")
        .arg(&config.connection_string)
        .arg("--db")
        .arg(database)
        .arg("--out")
        .arg(output_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute mongodump")?;

    progress.finish_with_message("Export completed");

    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr)?;
        error!("Export failed: {}", stderr);
        anyhow::bail!("Export failed: {}", stderr);
    } else {
        let stdout = str::from_utf8(&output.stdout)?;
        info!("Export output: {}", stdout);
    }

    // Verify that the export directory was created
    let db_path = output_dir.join(database);
    if !db_path.exists() {
        error!("Export directory not found: {}", db_path.display());
        anyhow::bail!("Export directory not found: {}", db_path.display());
    }

    Ok(())
}

pub async fn import_database(
    config: &MongoConfig,
    database: &str,
    input_dir: &Path,
    drop: bool,
    clear: bool,
) -> Result<()> {
    info!("Importing database {} to {}", database, config.environment);

    // If clear is true but drop is false, clear all collections first
    if clear && !drop {
        clear_collections(config, database).await?;
    }

    let progress = create_progress_bar("Importing");

    let bin_path = get_mongodb_bin_path().map_err(|e| {
        error!("Failed to find MongoDB tools: {}", e);
        anyhow::anyhow!("Failed to find mongorestore")
    })?;
    let mongorestore_path = bin_path.join("mongorestore");

    info!("Using mongorestore from: {}", mongorestore_path.display());

    // Verify that the database directory exists in the input directory
    let db_path = input_dir.join(database);
    if !db_path.exists() {
        error!("Database directory not found: {}", db_path.display());
        anyhow::bail!("Database directory not found: {}", db_path.display());
    }

    // Build the restore command using --nsInclude instead of deprecated --db flag
    let mut command = Command::new(&mongorestore_path);
    command
        .arg("--uri")
        .arg(&config.connection_string)
        .arg("--nsInclude")
        .arg(format!("{}.*", database));

    if drop {
        command.arg("--drop");
    }

    // Pass parent directory - mongorestore expects structure: input_dir/database/collection.bson
    command.arg(input_dir);

    info!("Running restore with directory: {}", input_dir.display());

    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("Failed to execute mongorestore")?;

    progress.finish_with_message("Import completed");

    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr)?;
        error!("Import failed: {}", stderr);
        anyhow::bail!("Import failed: {}", stderr);
    } else {
        let stdout = str::from_utf8(&output.stdout)?;
        info!("Import output: {}", stdout);
    }

    Ok(())
}

pub async fn create_backup(config: &MongoConfig, database: &str) -> Result<std::path::PathBuf> {
    info!(
        "Creating backup of {} from {}",
        database, config.environment
    );

    let backup_dir = get_backup_dir();
    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let backup_path = backup_dir.join(format!("backup_{}_{}", database, timestamp));

    std::fs::create_dir_all(&backup_path)?;

    export_database(config, database, &backup_path).await?;

    Ok(backup_path)
}

pub async fn restore_backup(
    config: &MongoConfig,
    database: &str,
    backup_path: &Path,
) -> Result<()> {
    info!("Restoring backup of {} to {}", database, config.environment);

    // Always use drop=true when restoring a backup to ensure complete restore
    import_database(config, database, backup_path, true, false).await?;

    Ok(())
}

pub async fn clear_collections(config: &MongoConfig, database: &str) -> Result<()> {
    info!(
        "Clearing all collections in database {} on {}",
        database, config.environment
    );

    let progress = create_progress_bar("Clearing collections");

    let client_options = config.get_client_options().await?;
    let client = mongodb::Client::with_options(client_options)?;
    let db = client.database(database);

    // Get all collections in the database
    let mut collections = db.list_collection_names().await?;

    // Remove system collections
    collections.retain(|name| !name.starts_with("system."));

    // Clear each collection by deleting all documents
    for collection_name in collections {
        let collection = db.collection::<mongodb::bson::Document>(&collection_name);
        collection.delete_many(mongodb::bson::doc! {}).await?;
    }

    progress.finish_with_message("Collections cleared");

    Ok(())
}

fn create_progress_bar(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message(format!("{} in progress...", message));
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    pb
}
