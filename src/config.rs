//! Configuration loading, saving, and initialization.

use crate::paths::{config_file, now_string};
use crate::project::Project;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;

/// On-disk configuration schema.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Config {
    /// Schema version.
    pub version: u32,
    /// Indexed Lean projects.
    pub projects: Vec<Project>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            projects: Vec::new(),
        }
    }
}

/// Create the default configuration file if it does not already exist.
pub fn init_config() -> Result<()> {
    let path = config_file()?;
    if path.exists() {
        bail!("config already exists at {}", path.display());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let config = Config::default();
    let content = serde_json::to_string_pretty(&config)?;
    fs::write(&path, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;

    println!("Created {}", path.display());
    Ok(())
}

/// Load configuration, returning an empty default when no config exists.
pub fn load_config() -> Result<Config> {
    let path = config_file()?;
    if !path.exists() {
        return Ok(Config::default());
    }

    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let config: Config = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if config.version != 1 {
        bail!("unsupported config version {}", config.version);
    }
    Ok(config)
}

/// Save configuration to the default path.
pub fn save_config(config: &Config) -> Result<()> {
    let path = config_file()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    fs::write(&path, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Update last-seen timestamps for projects that still exist.
pub fn touch_seen(config: &mut Config) {
    let now = now_string();
    for project in &mut config.projects {
        if project.expanded_path().exists() {
            project.last_seen_at = Some(now.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Project.last_committed_at` carries a `serde(alias = "last_commit_at")`
    /// so configs written before the rename keep loading. Both spellings must
    /// populate the same field.
    #[test]
    fn legacy_last_commit_at_alias_loads() {
        let legacy = r#"{
            "version": 1,
            "projects": [{
                "name": "demo",
                "path": "/tmp/demo",
                "last_commit_at": "1700000000"
            }]
        }"#;
        let config: Config = serde_json::from_str(legacy).unwrap();
        assert_eq!(config.projects.len(), 1);
        assert_eq!(
            config.projects[0].last_committed_at.as_deref(),
            Some("1700000000")
        );
    }

    /// Canonical `last_committed_at` spelling still loads after the alias is
    /// added; the alias must not steal the canonical name.
    #[test]
    fn canonical_last_committed_at_still_loads() {
        let canonical = r#"{
            "version": 1,
            "projects": [{
                "name": "demo",
                "path": "/tmp/demo",
                "last_committed_at": "1700000000"
            }]
        }"#;
        let config: Config = serde_json::from_str(canonical).unwrap();
        assert_eq!(
            config.projects[0].last_committed_at.as_deref(),
            Some("1700000000")
        );
    }

    /// Loading the shipped example config must succeed with the current
    /// schema. CI uses this to catch future drift between the schema and
    /// its canonical example.
    #[test]
    fn examples_config_loads() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("config.json");
        let content = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let config: Config = serde_json::from_str(&content)
            .unwrap_or_else(|error| panic!("example config is invalid: {error}"));
        assert_eq!(config.version, 1);
        assert!(!config.projects.is_empty());
    }
}
