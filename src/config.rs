//! Configuration loading, saving, and initialization.

use crate::paths::{config_file, now_string};
use crate::project::Project;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
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
/// The JSON is written to a temporary file beside the real target, synced,
/// and renamed over it, so an interrupted save (Ctrl-C, full disk, crash)
/// leaves the previous index intact instead of a truncated file. When
/// `config.json` is a symlink (as dotfile managers create), the link's
/// target is replaced and the link itself is kept; the target's
/// permissions carry over to the new file.
fn write_config_to(path: &Path, config: &Config) -> Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let content = format!("{}\n", serde_json::to_string_pretty(config)?);
    let mut tmp_name = target.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = target.with_file_name(tmp_name);

    let result =
        write_synced(&tmp, content.as_bytes(), fs::metadata(&target).ok()).and_then(|()| {
            fs::rename(&tmp, &target)
                .with_context(|| format!("failed to replace {}", target.display()))
        });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Create `path` with `content`, flushed to disk. Permissions are copied
/// from `like` before any content is written, so a private (0600) config
/// is never briefly world-readable.
fn write_synced(path: &Path, content: &[u8], like: Option<fs::Metadata>) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    if let Some(metadata) = like {
        // Best effort: a filesystem that cannot set modes still gets the save.
        let _ = fs::set_permissions(path, metadata.permissions());
    }
    file.write_all(content)
        .and_then(|()| file.sync_all())
        .with_context(|| format!("failed to write {}", path.display()))
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

    /// Dotfile managers often symlink `config.json`. Saving must update the
    /// link's target, keep the link, and keep the target's permissions.
    #[cfg(unix)]
    #[test]
    fn write_config_to_writes_through_symlinks_and_keeps_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("leanmgr-config-symlink-{nonce}"));
        fs::create_dir_all(dir.join("dotfiles")).unwrap();
        let real = dir.join("dotfiles").join("leanmgr.json");
        fs::write(&real, "{\"version\": 1, \"projects\": []}\n").unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.join("config.json");
        symlink(&real, &link).unwrap();

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
        write_config_to(&link, &config).unwrap();

        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let saved: Config = serde_json::from_str(&fs::read_to_string(&real).unwrap()).unwrap();
        assert_eq!(saved.projects.len(), 1);
        let mode = fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let leftovers: Vec<_> = fs::read_dir(dir.join("dotfiles"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("leanmgr.json")]);

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
