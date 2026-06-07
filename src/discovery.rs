//! Recursive Lake project discovery.

use crate::cli::ScanArgs;
use crate::config::{load_config, save_config};
use crate::output::print_json;
use crate::paths::{display_path, expand_tilde, normalize_existing_or_join, now_string};
use crate::project::Project;
use anyhow::Result;
use serde::Serialize;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

/// Discovered project candidate.
#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredProject {
    /// Project path.
    pub path: String,
    /// Suggested project name.
    pub name: String,
}

/// Run the scan command.
pub fn scan_command(args: ScanArgs) -> Result<()> {
    let root = normalize_existing_or_join(&expand_tilde(&args.root))?;
    let discovered = scan_projects(&root)?;

    if args.json {
        return print_json(&discovered);
    }

    for project in &discovered {
        println!("{}  {}", project.name, project.path);
    }

    if args.yes {
        add_discovered(discovered)?;
    } else {
        println!("Run with --yes to add discovered projects.");
    }

    Ok(())
}

/// Find Lake projects under a root directory.
pub fn scan_projects(root: &Path) -> Result<Vec<DiscoveredProject>> {
    let mut projects = Vec::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_pruned(entry));

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_dir() {
            continue;
        }
        let path = entry.path();
        if path.join("lakefile.toml").exists() || path.join("lakefile.lean").exists() {
            projects.push(discovered_from_path(path));
        }
    }

    projects.sort_by(|left, right| left.path.cmp(&right.path));
    projects.dedup_by(|left, right| left.path == right.path);
    Ok(projects)
}

fn add_discovered(discovered: Vec<DiscoveredProject>) -> Result<()> {
    let mut config = load_config()?;
    let now = now_string();
    let mut added = 0;
    for item in discovered {
        if config
            .projects
            .iter()
            .any(|project| project.path == item.path)
        {
            continue;
        }
        let name = unique_name(&config.projects, &item.name);
        config.projects.push(Project {
            name,
            path: item.path,
            tags: Vec::new(),
            description: None,
            added_at: Some(now.clone()),
            last_seen_at: Some(now.clone()),
            size_cache: None,
        });
        added += 1;
    }
    save_config(&config)?;
    println!("Added {added} project(s)");
    Ok(())
}

/// Return a project name that does not collide with existing names.
///
/// Distinct directories can share a basename, so a suffix is appended when the
/// suggested name is already taken rather than silently dropping the project.
fn unique_name(projects: &[Project], base: &str) -> String {
    if !projects.iter().any(|project| project.name == base) {
        return base.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{base}-{suffix}");
        if !projects.iter().any(|project| project.name == candidate) {
            return candidate;
        }
        suffix += 1;
    }
}

fn discovered_from_path(path: &Path) -> DiscoveredProject {
    let path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string());
    DiscoveredProject {
        path: display_path(&path),
        name,
    }
}

fn is_pruned(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    matches!(name.as_ref(), ".git" | ".lake" | "target" | "node_modules")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn scan_finds_lake_projects_and_skips_lake_cache() {
        let root = test_dir("scan_finds_lake_projects");
        fs::create_dir_all(root.join("A/.lake/nested")).unwrap();
        fs::create_dir_all(root.join("B")).unwrap();
        fs::write(root.join("A/lakefile.toml"), b"name = \"A\"").unwrap();
        fs::write(root.join("A/.lake/nested/lakefile.toml"), b"name = \"bad\"").unwrap();
        fs::write(root.join("B/lakefile.lean"), b"import Lake").unwrap();

        let found = scan_projects(&root).unwrap();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|project| project.name == "A"));
        assert!(found.iter().any(|project| project.name == "B"));

        fs::remove_dir_all(root).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }

    fn named(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn unique_name_suffixes_collisions() {
        let projects = vec![named("demo"), named("demo-2")];
        assert_eq!(unique_name(&projects, "fresh"), "fresh");
        assert_eq!(unique_name(&projects, "demo"), "demo-3");
    }
}
