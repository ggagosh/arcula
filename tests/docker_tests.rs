use std::env;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use ::mongodb::bson::{doc, Document};
use ::mongodb::Client;
use anyhow::Result;
use arcula::config::{Environment, MongoConfig};
use arcula::core::sync::{SyncConfig, SyncOptions};
use arcula::utils::mongodb;

// This file contains integration tests that use real MongoDB instances
// It uses Docker to spin up temporary MongoDB containers for testing

// Get the IP address of a Docker container by name
fn get_container_ip(container_name: &str) -> Result<String> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "-f",
            "{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}",
            container_name,
        ])
        .output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("Failed to get container IP address"));
    }

    let ip = String::from_utf8(output.stdout)?.trim().to_string();

    if ip.is_empty() {
        return Err(anyhow::anyhow!("Container IP address not found"));
    }

    Ok(ip)
}

// Use a function to generate unique container names for each test
fn generate_container_names() -> (String, String) {
    // Generate a unique suffix based on timestamp and random number
    let suffix = format!(
        "{}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        rand::random::<u16>()
    );

    (
        format!("mongo_importer_test_source_{}", suffix),
        format!("mongo_importer_test_target_{}", suffix),
    )
}

// Environment variables for CI environment
const ENV_MONGO_SOURCE_URI: &str = "TEST_MONGO_SOURCE_URI";
const ENV_MONGO_TARGET_URI: &str = "TEST_MONGO_TARGET_URI";

// Setup function to start MongoDB containers with unique names and get their IPs
fn setup_mongodb_containers() -> Result<((String, String), (String, String))> {
    // Check if Docker is available
    let docker_check = Command::new("docker").arg("--version").output()?;

    if !docker_check.status.success() {
        eprintln!("Docker is not available.");
        return Err(anyhow::anyhow!("Docker is not available"));
    }

    // Generate unique container names for this test run
    let (container_name1, container_name2) = generate_container_names();

    // Start the source MongoDB container
    let start_source = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-d",
            "--name",
            &container_name1,
            "mongo:latest",
        ])
        .stdout(Stdio::null())
        .status()?;

    if !start_source.success() {
        return Err(anyhow::anyhow!("Failed to start source MongoDB container"));
    }

    // Start the target MongoDB container
    let start_target = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-d",
            "--name",
            &container_name2,
            "mongo:latest",
        ])
        .stdout(Stdio::null())
        .status()?;

    if !start_target.success() {
        // Clean up the first container if the second fails
        let _ = Command::new("docker")
            .args(["rm", "-f", &container_name1])
            .stdout(Stdio::null())
            .status();
        return Err(anyhow::anyhow!("Failed to start target MongoDB container"));
    }

    // Wait for MongoDB to be ready
    println!("Waiting for MongoDB containers to be ready...");
    thread::sleep(Duration::from_secs(5));

    // Get container IP addresses
    let ip1 = get_container_ip(&container_name1)?;
    let ip2 = get_container_ip(&container_name2)?;

    println!("MongoDB containers running at IPs {} and {}", ip1, ip2);

    Ok(((container_name1, container_name2), (ip1, ip2)))
}

// Teardown function to stop and remove MongoDB containers
fn teardown_mongodb_containers(container_names: &(String, String)) -> Result<()> {
    // Stop and remove the containers
    let _ = Command::new("docker")
        .args(["rm", "-f", &container_names.0])
        .stdout(Stdio::null())
        .status();

    let _ = Command::new("docker")
        .args(["rm", "-f", &container_names.1])
        .stdout(Stdio::null())
        .status();

    Ok(())
}

/// Create MongoDB configurations for testing
///
/// This function will use connection strings from environment variables if they exist,
/// otherwise it will use the provided container IPs.
fn get_test_configs(ips: Option<(String, String)>) -> (MongoConfig, MongoConfig) {
    // Check if connection strings are provided via environment variables
    let source_uri = env::var(ENV_MONGO_SOURCE_URI).unwrap_or_else(|_| {
        let ip = ips
            .as_ref()
            .map(|(ip, _)| ip.clone())
            .unwrap_or_else(|| "localhost".to_string());
        format!("mongodb://{}:27017", ip)
    });

    let target_uri = env::var(ENV_MONGO_TARGET_URI).unwrap_or_else(|_| {
        let ip = ips
            .as_ref()
            .map(|(_, ip)| ip.clone())
            .unwrap_or_else(|| "localhost".to_string());
        format!("mongodb://{}:27017", ip)
    });

    let source_config = MongoConfig {
        connection_string: source_uri,
        environment: Environment::new("TEST_SOURCE"),
    };

    let target_config = MongoConfig {
        connection_string: target_uri,
        environment: Environment::new("TEST_TARGET"),
    };

    (source_config, target_config)
}

// Helper function to create test data in source MongoDB
async fn create_test_data(config: &MongoConfig, db_name: &str) -> Result<()> {
    // Use the MongoDB client to create test data
    let client_options = config.get_client_options().await?;
    let client = Client::with_options(client_options)?;
    let db = client.database(db_name);
    let collection = db.collection::<Document>("test_collection");

    // Create test documents
    for i in 0..10 {
        let doc = doc! {
            "test_field": format!("test_value_{}", i),
            "test_number": i
        };
        collection.insert_one(doc).await?;
    }

    Ok(())
}

// Helper function to verify data was synced correctly
async fn verify_synced_data(config: &MongoConfig, db_name: &str) -> Result<bool> {
    // Use the MongoDB client to verify the data
    let client_options = config.get_client_options().await?;
    let client = Client::with_options(client_options)?;
    let db = client.database(db_name);
    let collection = db.collection::<Document>("test_collection");

    // Count documents
    let count = collection.count_documents(doc! {}).await?;

    Ok(count == 10)
}

// Test MongoDB connection
#[tokio::test]
async fn test_mongodb_connection() -> Result<()> {
    // Check if we have MongoDB URIs configured in environment
    let external_mongo =
        env::var(ENV_MONGO_SOURCE_URI).is_ok() && env::var(ENV_MONGO_TARGET_URI).is_ok();

    // Container names and IPs to be used for cleanup if needed
    let mut container_info = None;

    // Setup Docker containers if needed
    if !external_mongo {
        match setup_mongodb_containers() {
            Ok((container_names, ips)) => {
                container_info = Some((container_names, ips));
            }
            Err(e) => {
                eprintln!("Error setting up MongoDB containers: {}", e);
                return Err(anyhow::anyhow!(
                    "Failed to set up MongoDB containers: {}",
                    e
                ));
            }
        }
    }

    // Run the test
    let (source_config, target_config) =
        get_test_configs(container_info.as_ref().map(|(_, ips)| ips.clone()));

    // Test that we can connect to both MongoDB instances
    let source_dbs = mongodb::list_databases(&source_config).await?;
    let target_dbs = mongodb::list_databases(&target_config).await?;

    println!("Source DBs: {:?}", source_dbs);
    println!("Target DBs: {:?}", target_dbs);

    // Ensure both MongoDB instances are running
    assert!(source_dbs.contains(&"admin".to_string()));
    assert!(target_dbs.contains(&"admin".to_string()));

    // Teardown MongoDB containers if we created them
    if !external_mongo && container_info.is_some() {
        teardown_mongodb_containers(&container_info.unwrap().0)?;
    }

    Ok(())
}

// Test export and import functionality
#[tokio::test]
async fn test_export_import() -> Result<()> {
    // Check if we have MongoDB URIs configured in environment
    let external_mongo =
        env::var(ENV_MONGO_SOURCE_URI).is_ok() && env::var(ENV_MONGO_TARGET_URI).is_ok();

    // Container names and IPs to be used for cleanup if needed
    let mut container_info = None;

    // Setup Docker containers if needed
    if !external_mongo {
        match setup_mongodb_containers() {
            Ok((container_names, ips)) => {
                container_info = Some((container_names, ips));
            }
            Err(e) => {
                eprintln!("Error setting up MongoDB containers: {}", e);
                return Err(anyhow::anyhow!(
                    "Failed to set up MongoDB containers: {}",
                    e
                ));
            }
        }
    }

    // Get MongoDB configs
    let (source_config, target_config) =
        get_test_configs(container_info.as_ref().map(|(_, ips)| ips.clone()));

    // Create test database and collection
    let test_db = "test_db";
    create_test_data(&source_config, test_db).await?;

    // Create temporary directory for the export/import
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path();

    // Export the database
    let export_result = mongodb::export_database(&source_config, test_db, temp_path).await;
    assert!(export_result.is_ok());

    // Import the database to the target
    let import_result =
        mongodb::import_database(&target_config, test_db, temp_path, true, false).await;
    assert!(import_result.is_ok());

    // Verify the data was imported correctly
    let verification = verify_synced_data(&target_config, test_db).await?;
    assert!(verification);

    // Teardown MongoDB containers if we created them
    if !external_mongo && container_info.is_some() {
        teardown_mongodb_containers(&container_info.unwrap().0)?;
    }

    Ok(())
}

// Test backup and restore functionality
#[tokio::test]
async fn test_backup_restore() -> Result<()> {
    // Check if we have MongoDB URIs configured in environment
    let external_mongo =
        env::var(ENV_MONGO_SOURCE_URI).is_ok() && env::var(ENV_MONGO_TARGET_URI).is_ok();

    // Container names and IPs to be used for cleanup if needed
    let mut container_info = None;

    // Setup Docker containers if needed
    if !external_mongo {
        match setup_mongodb_containers() {
            Ok((container_names, ips)) => {
                container_info = Some((container_names, ips));
            }
            Err(e) => {
                eprintln!("Error setting up MongoDB containers: {}", e);
                return Err(anyhow::anyhow!(
                    "Failed to set up MongoDB containers: {}",
                    e
                ));
            }
        }
    }

    // Get MongoDB configs
    let (source_config, _) = get_test_configs(container_info.as_ref().map(|(_, ips)| ips.clone()));

    // Create test database and collection
    let test_db = "backup_test_db";
    create_test_data(&source_config, test_db).await?;

    // Create a backup
    let backup_result = mongodb::create_backup(&source_config, test_db).await;
    assert!(backup_result.is_ok());
    let backup_path = backup_result.unwrap();

    // Clear the database
    let client_options = source_config.get_client_options().await?;
    let client = Client::with_options(client_options)?;
    client.database(test_db).drop().await?;

    // Restore from backup
    let restore_result = mongodb::restore_backup(&source_config, test_db, &backup_path).await;
    assert!(restore_result.is_ok());

    // Verify the data was restored correctly
    let verification = verify_synced_data(&source_config, test_db).await?;
    assert!(verification);

    // Teardown MongoDB containers if we created them
    if !external_mongo && container_info.is_some() {
        teardown_mongodb_containers(&container_info.unwrap().0)?;
    }

    Ok(())
}

// Test the full sync operation
#[tokio::test]
async fn test_full_sync_operation() -> Result<()> {
    // Check if we have MongoDB URIs configured in environment
    let external_mongo =
        env::var(ENV_MONGO_SOURCE_URI).is_ok() && env::var(ENV_MONGO_TARGET_URI).is_ok();

    // Container names and IPs to be used for cleanup if needed
    let mut container_info = None;

    // Setup Docker containers if needed
    if !external_mongo {
        match setup_mongodb_containers() {
            Ok((container_names, ips)) => {
                container_info = Some((container_names, ips));
            }
            Err(e) => {
                eprintln!("Error setting up MongoDB containers: {}", e);
                return Err(anyhow::anyhow!(
                    "Failed to set up MongoDB containers: {}",
                    e
                ));
            }
        }
    }

    // Get MongoDB configs
    let (source_config, target_config) =
        get_test_configs(container_info.as_ref().map(|(_, ips)| ips.clone()));

    // Create test database and collection
    let source_db = "sync_source_db";
    let target_db = "sync_target_db";
    create_test_data(&source_config, source_db).await?;

    // Create sync config
    let sync_config = SyncConfig {
        source_env: source_config.environment.clone(),
        target_env: target_config.environment.clone(),
        source_db: source_db.to_string(),
        target_db: target_db.to_string(),
        options: SyncOptions {
            create_backup: true,
            drop_collections: true,
            clear_collections: false,
        },
    };

    // Set environment variables for the config
    env::set_var("MONGO_TEST_SOURCE_URI", &source_config.connection_string);
    env::set_var("MONGO_TEST_TARGET_URI", &target_config.connection_string);

    // Perform the sync
    let sync_result = arcula::core::sync::perform_sync(sync_config).await;
    assert!(sync_result.is_ok());

    // Verify the data was synced correctly
    let verification = verify_synced_data(&target_config, target_db).await?;
    assert!(verification);

    // Clean up environment variables
    env::remove_var("MONGO_TEST_SOURCE_URI");
    env::remove_var("MONGO_TEST_TARGET_URI");

    // Teardown MongoDB containers if we created them
    if !external_mongo && container_info.is_some() {
        teardown_mongodb_containers(&container_info.unwrap().0)?;
    }

    Ok(())
}
