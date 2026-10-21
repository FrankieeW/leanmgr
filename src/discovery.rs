//! Recursive Lake project discovery.

use crate::cli::ScanArgs;
use crate::config::{load_config, save_config};
use crate::lake::has_lakefile;
use crate::output::print_json;
use crate::paths::{display_path, expand_tilde, normalize_existing_or_join, now_string};
use crate::project::Project;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::HashSet;
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
    if !root.is_dir() {
        bail!("scan root is not a directory: {}", root.display());
    }
    let scan = scan_projects(&root);
    if scan.unreadable > 0 {
        // stderr keeps `--json` output on stdout machine-readable.
        eprintln!(
            "Skipped {} unreadable path(s) under {}",
            scan.unreadable,
            root.display()
        );
    }
    let discovered = scan.projects;

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

/// Lake projects found under a scan root.
#[derive(Clone, Debug, Default)]
pub struct ScanOutcome {
    /// Discovered projects, sorted by path.
    pub projects: Vec<DiscoveredProject>,
    /// Entries that could not be read and were skipped.
    pub unreadable: usize,
}

/// Find Lake projects under a root directory.
///
/// Unreadable directories are counted and skipped instead of aborting the
/// scan, so `scan ~` still works when some folders are off limits (for
/// example macOS privacy-protected folders such as `~/Library/Mail`).
pub fn scan_projects(root: &Path) -> ScanOutcome {
    let mut outcome = ScanOutcome::default();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !is_pruned(entry));

    for entry in walker {
        let Ok(entry) = entry else {
            outcome.unreadable += 1;
            continue;
        };
        if entry.file_type().is_dir() && has_lakefile(entry.path()) {
            outcome.projects.push(discovered_from_path(entry.path()));
        }
    }

    outcome
        .projects
        .sort_by(|left, right| left.path.cmp(&right.path));
    outcome
        .projects
        .dedup_by(|left, right| left.path == right.path);
    outcome
}

fn add_discovered(discovered: Vec<DiscoveredProject>) -> Result<()> {
    let mut config = load_config()?;
    let added = merge_discovered(&mut config.projects, discovered, &now_string());
    save_config(&config)?;
    println!("Added {added} project(s)");
    Ok(())
}

/// Append discovered projects that are not indexed yet; return how many
/// were added.
///
/// Existing entries are compared by canonical path, so an entry stored as
/// `~/LeanProjects/Foo` is recognized as the same project the scan found
/// at `/Users/me/LeanProjects/Foo`.
fn merge_discovered(
    projects: &mut Vec<Project>,
    discovered: Vec<DiscoveredProject>,
    now: &str,
) -> usize {
    let mut indexed: HashSet<PathBuf> = projects
        .iter()
        .map(|project| canonical_or_raw(&project.expanded_path()))
        .collect();
    let mut added = 0;
    for item in discovered {
        if !indexed.insert(canonical_or_raw(Path::new(&item.path))) {
            continue;
        }
        let name = unique_name(projects, &item.name);
        projects.push(Project {
            name,
            path: item.path,
            tags: Vec::new(),
            description: None,
            added_at: Some(now.to_string()),
            last_seen_at: Some(now.to_string()),
            last_committed_at: Some(now.to_string()),
            size_cache: None,
        });
        added += 1;
    }
    added
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
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

        let found = scan_projects(&root).projects;
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|project| project.name == "A"));
        assert!(found.iter().any(|project| project.name == "B"));

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_skips_unreadable_directories() {
        use std::os::unix::fs::PermissionsExt;
        let root = test_dir("scan_skips_unreadable");
        fs::create_dir_all(root.join("A")).unwrap();
        fs::create_dir_all(root.join("locked/C")).unwrap();
        fs::write(root.join("A/lakefile.toml"), b"name = \"A\"").unwrap();
        fs::write(root.join("locked/C/lakefile.toml"), b"name = \"C\"").unwrap();
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();

        let scan = scan_projects(&root);

        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&root).unwrap();
        assert!(scan.projects.iter().any(|project| project.name == "A"));
        // Root bypasses permission bits; only assert the skip when it applied.
        if !scan.projects.iter().any(|project| project.name == "C") {
            assert_eq!(scan.unreadable, 1);
        }
    }

    #[test]
    fn merge_discovered_matches_existing_entries_by_canonical_path() {
        let root = test_dir("merge_discovered_canonical");
        fs::create_dir_all(root.join("A")).unwrap();
        let canonical = root.join("A").canonicalize().unwrap();
        // The index stores a non-canonical spelling of the same directory.
        let mut existing = named("A");
        existing.path = display_path(&root.join("A").join("..").join("A"));
        let mut projects = vec![existing];

        let discovered = vec![
            DiscoveredProject {
                path: display_path(&canonical),
                name: "A".to_string(),
            },
            DiscoveredProject {
                path: "/tmp/leanmgr-merge-new-project".to_string(),
                name: "New".to_string(),
            },
        ];
        let added = merge_discovered(&mut projects, discovered, "1");

        assert_eq!(added, 1);
        assert_eq!(projects.len(), 2);
        assert_eq!(projects[1].name, "New");
        assert_eq!(projects[1].added_at.as_deref(), Some("1"));
        fs::remove_dir_all(&root).unwrap();
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
            last_committed_at: None,
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
