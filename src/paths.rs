//! Path helpers for cross-platform config and project handling.

use anyhow::{Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Return the default configuration file path.
pub fn config_file() -> Result<PathBuf> {
    if cfg!(windows) {
        let appdata = env::var_os("APPDATA").context("APPDATA is not set")?;
        return Ok(PathBuf::from(appdata).join("leanmgr").join("config.json"));
    }

    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("leanmgr").join("config.json"));
    }

    Ok(home_dir()?
        .join(".config")
        .join("leanmgr")
        .join("config.json"))
}

/// Resolve a leading `~` to the current user's home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir().unwrap_or_else(|_| PathBuf::from(path));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|_| PathBuf::from(path));
    }
    PathBuf::from(path)
}

/// Convert a path to a stored string.
pub fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Return a simple UTC-ish timestamp string as seconds since epoch.
pub fn now_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("USERPROFILE").map(PathBuf::from))
        .ok_or_else(|| anyhow::anyhow!("could not determine home directory"))
}

/// Return a canonical path when possible, otherwise an absolute-ish path.
pub fn normalize_existing_or_join(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return path
            .canonicalize()
            .with_context(|| format!("failed to canonicalize {}", path.display()));
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(env::current_dir()?.join(path))
}
