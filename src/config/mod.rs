use std::env;
use std::path::PathBuf;

use mongodb::options::ClientOptions;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Environment variable not found: {0}")]
    EnvVarNotFound(String),

    #[error("Invalid environment: {0}")]
    InvalidEnvironment(String),

    #[error("MongoDB connection error: {0}")]
    MongoDBConnection(#[from] mongodb::error::Error),

    #[error("Failed to locate MongoDB binary: {0}")]
    WhichError(#[from] which::Error),

    #[error("MongoDB binary not found")]
    BinaryNotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Environment(String);

impl Environment {
    pub fn new(name: &str) -> Self {
        Self(name.to_uppercase())
    }

    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Environment {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.trim().is_empty() {
            return Err(ConfigError::InvalidEnvironment(
                "Empty environment name".to_string(),
            ));
        }
        Ok(Self::new(s))
    }
}

#[derive(Debug, Clone)]
pub struct MongoConfig {
    pub connection_string: String,
    pub environment: Environment,
}

impl MongoConfig {
    pub fn from_env(env: Environment) -> Result<Self, ConfigError> {
        let var_name = format!("MONGO_{}_URI", env);
        let connection_string =
            env::var(&var_name).map_err(|_| ConfigError::EnvVarNotFound(var_name))?;

        Ok(Self {
            connection_string,
            environment: env,
        })
    }

    pub async fn get_client_options(&self) -> Result<ClientOptions, ConfigError> {
        let options = ClientOptions::parse(&self.connection_string).await?;
        Ok(options)
    }
}

pub fn get_mongodb_bin_path() -> Result<PathBuf, ConfigError> {
    if let Ok(path) = env::var("MONGODB_BIN_PATH") {
        let path_buf = PathBuf::from(&path);
        let mongodump_exists = path_buf.join("mongodump").exists();
        let mongorestore_exists = path_buf.join("mongorestore").exists();

        if mongodump_exists && mongorestore_exists {
            return Ok(path_buf);
        }

        let mut missing = Vec::new();
        if !mongodump_exists {
            missing.push("mongodump");
        }
        if !mongorestore_exists {
            missing.push("mongorestore");
        }

        return Err(ConfigError::InvalidEnvironment(format!(
            "MONGODB_BIN_PATH='{}' missing: {}",
            path,
            missing.join(", ")
        )));
    }

    // Try to find mongodump in PATH using 'which'
    if let Ok(mongodump_path) = which::which("mongodump") {
        if let Some(parent) = mongodump_path.parent() {
            // Verify mongorestore exists in the same directory
            if parent.join("mongorestore").exists() {
                return Ok(parent.to_path_buf());
            }
        }
    }

    // If we get here, we couldn't find the binaries
    Err(ConfigError::BinaryNotFound)
}

/// Checks if MongoDB tools (mongodump and mongorestore) are available
pub fn check_mongodb_tools() -> Result<(), ConfigError> {
    // This will return an error if it can't find both mongodump and mongorestore
    get_mongodb_bin_path().map(|_| ())
}

/// Get all available MongoDB environments from environment variables
pub fn get_available_environments() -> Vec<Environment> {
    let prefix = "MONGO_";
    let suffix = "_URI";

    // Get all environment variables
    let mut environments = Vec::new();

    for (key, _) in env::vars() {
        if key.starts_with(prefix) && key.ends_with(suffix) {
            // Extract the environment name (between MONGO_ and _URI)
            if let Some(env_name) = key
                .strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(suffix))
            {
                if !env_name.is_empty() {
                    environments.push(Environment::new(env_name));
                }
            }
        }
    }

    // Sort environments alphabetically for consistent display
    environments.sort_by(|a, b| a.name().cmp(b.name()));

    environments
}

pub fn get_backup_dir() -> PathBuf {
    env::var("BACKUP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let mut path = env::temp_dir();
            path.push("mongo_importer_backups");
            path
        })
}
