//! `.gitignore` maintenance for Lean projects.

use crate::cli::GitignoreArgs;
use crate::project::{Project, select_projects};
use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const LAKE_IGNORE_ENTRY: &str = ".lake/";

/// One planned `.gitignore` update.
#[derive(Clone, Debug, Serialize)]
pub struct GitignoreUpdate {
    /// Project name.
    pub project: String,
    /// `.gitignore` path.
    pub path: PathBuf,
    /// Whether a write is needed.
    pub changed: bool,
}

/// Run the gitignore command.
pub fn gitignore_command(args: GitignoreArgs) -> Result<()> {
    let projects = select_projects(args.project.as_deref(), args.tag.as_deref(), args.all)?;
    let updates = plan_gitignore_updates(&projects)?;

    for update in &updates {
        let action = if update.changed {
            "add .lake/"
        } else {
            "already ok"
        };
        println!("{}  {}  {}", update.project, action, update.path.display());
    }

    if args.dry_run {
        return Ok(());
    }

    for update in updates.iter().filter(|update| update.changed) {
        apply_gitignore_update(&update.path)?;
    }
    Ok(())
}

/// Plan `.gitignore` updates for projects.
pub fn plan_gitignore_updates(projects: &[Project]) -> Result<Vec<GitignoreUpdate>> {
    projects
        .iter()
        .map(|project| {
            let path = project.expanded_path().join(".gitignore");
            Ok(GitignoreUpdate {
                project: project.name.clone(),
                changed: !gitignore_has_lake(&path)?,
                path,
            })
        })
        .collect()
}

fn gitignore_has_lake(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(content
        .lines()
        .map(str::trim)
        .any(|line| matches!(line, ".lake/" | ".lake")))
}

fn apply_gitignore_update(path: &Path) -> Result<()> {
    let mut content = if path.exists() {
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };

    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(LAKE_IGNORE_ENTRY);
    content.push('\n');

    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn gitignore_update_is_idempotent() {
        let root = test_dir("gitignore_update_is_idempotent");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(".gitignore");

        apply_gitignore_update(&path).unwrap();
        assert!(gitignore_has_lake(&path).unwrap());
        let once = fs::read_to_string(&path).unwrap();

        if !gitignore_has_lake(&path).unwrap() {
            apply_gitignore_update(&path).unwrap();
        }
        let twice = fs::read_to_string(&path).unwrap();
        assert_eq!(once, twice);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_existing_lake_without_slash() {
        let root = test_dir("detects_existing_lake_without_slash");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(".gitignore");
        fs::write(&path, ".lake\n").unwrap();

        assert!(gitignore_has_lake(&path).unwrap());

        fs::remove_dir_all(root).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }
}
