//! Configuration loading, saving, and initialization.

use crate::paths::{config_file, now_string};
use crate::project::Project;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

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

    write_config_to(&path, &Config::default())?;
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
    write_config_to(&config_file()?, config)
}

/// Write `config` to `path` atomically.
///
/// The JSON is written to a sibling temporary file and renamed over the
/// target, so an interrupted save (Ctrl-C, full disk, crash) leaves the
/// previous index intact instead of a truncated `config.json`.
fn write_config_to(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = serde_json::to_string_pretty(config)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&tmp, format!("{content}\n"))
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
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

    #[test]
    fn write_config_to_replaces_file_and_leaves_no_temp_files() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("leanmgr-config-atomic-{nonce}"));
        let path = dir.join("leanmgr").join("config.json");

        write_config_to(&path, &Config::default()).unwrap();
        let mut config = Config::default();
        config.projects.push(crate::project::Project {
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        });
        write_config_to(&path, &config).unwrap();

        let loaded: Config = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded.projects.len(), 1);
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("config.json")]);

        fs::remove_dir_all(&dir).unwrap();
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
