use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn data_dir() -> PathBuf {
    if let Ok(dir) = env::var("ARCULA_DATA_DIR") {
        return PathBuf::from(dir);
    }

    platform_data_dir().join("arcula")
}

pub fn plans_dir() -> PathBuf {
    data_dir().join("plans")
}

pub fn operations_dir() -> PathBuf {
    data_dir().join("operations")
}

pub fn ensure_parent(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    Ok(())
}

pub fn atomic_write_json<T: serde::Serialize>(path: &std::path::Path, value: &T) -> Result<()> {
    ensure_parent(path)?;
    let contents = serde_json::to_string_pretty(value)?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, contents)
        .with_context(|| format!("Failed to write {}", temp_path.display()))?;
    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "Failed to move {} to {}",
            temp_path.display(),
            path.display()
        )
    })?;
    Ok(())
}

fn platform_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
            return PathBuf::from(local_appdata);
        }
        if let Ok(appdata) = env::var("APPDATA") {
            return PathBuf::from(appdata);
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(home) = env::var("HOME") {
            return PathBuf::from(home)
                .join("Library")
                .join("Application Support");
        }
    }

    if let Ok(xdg_data_home) = env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg_data_home);
    }

    if let Ok(home) = env::var("HOME") {
        return PathBuf::from(home).join(".local").join("share");
    }

    PathBuf::from(".")
}
