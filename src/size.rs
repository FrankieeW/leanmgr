//! Disk usage accounting for `.lake` cache trees.

use crate::cli::SizeArgs;
use crate::config::load_config;
use crate::output::{format_bytes, print_json, print_table};
use crate::project::{Project, filter_by_tag, matches_project};
use anyhow::{Result, bail};
use serde::Serialize;
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

/// Size summary for one project.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ProjectSize {
    /// Project name.
    pub project: String,
    /// Bytes under `.lake`.
    pub lake: u64,
    /// Bytes under `.lake/build`.
    pub build: u64,
    /// Bytes under `.lake/packages`.
    pub packages: u64,
    /// Total bytes counted for the project.
    pub total: u64,
}

/// Multi-project size report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SizeReport {
    /// Per-project sizes.
    pub projects: Vec<ProjectSize>,
    /// Total bytes.
    pub total: u64,
}

/// Run the size command.
pub fn size_command(args: SizeArgs) -> Result<()> {
    let config = load_config()?;
    let selected = select_sized(
        &config.projects,
        args.project.as_deref(),
        args.tag.as_deref(),
    )?;

    let mut report = SizeReport::default();
    for project in selected {
        let size = project_size(project)?;
        report.total += size.total;
        report.projects.push(size);
    }

    if args.json {
        return print_json(&report);
    }

    let rows: Vec<Vec<String>> = report
        .projects
        .iter()
        .map(|size| {
            vec![
                size.project.clone(),
                format_bytes(size.lake),
                format_bytes(size.build),
                format_bytes(size.packages),
                format_bytes(size.total),
            ]
        })
        .collect();
    print_table(&["PROJECT", "LAKE", "BUILD", "PACKAGES", "TOTAL"], &rows);
    println!("Total: {}", format_bytes(report.total));
    Ok(())
}

/// Pick the projects a `size` invocation reports on.
///
/// A project selector must match an indexed project; an unknown name
/// fails loudly instead of printing an empty table. Without a selector,
/// the optional tag filter applies (all projects when absent).
fn select_sized<'a>(
    projects: &'a [Project],
    selector: Option<&str>,
    tag: Option<&str>,
) -> Result<Vec<&'a Project>> {
    let Some(selector) = selector else {
        return Ok(filter_by_tag(projects, tag));
    };
    let matched: Vec<&Project> = projects
        .iter()
        .filter(|project| matches_project(project, selector))
        .collect();
    if matched.is_empty() {
        bail!("project not found: {selector}");
    }
    Ok(matched)
}

/// Compute `.lake`-related size for one project.
///
/// Walks `.lake` once and buckets bytes by subtree. `build` and `packages` are
/// subtrees of `.lake`, so a single traversal avoids re-walking the bulk of the
/// cache three times.
pub fn project_size(project: &Project) -> Result<ProjectSize> {
    let root = project.expanded_path();
    let lake = root.join(".lake");
    let build = lake.join("build");
    let packages = lake.join("packages");

    let mut lake_size = 0;
    let mut build_size = 0;
    let mut packages_size = 0;

    if lake.exists() {
        for (entry, len) in files_with_sizes(&lake) {
            lake_size += len;
            if entry.path().starts_with(&build) {
                build_size += len;
            } else if entry.path().starts_with(&packages) {
                packages_size += len;
            }
        }
    }

    Ok(ProjectSize {
        project: project.name.clone(),
        lake: lake_size,
        build: build_size,
        packages: packages_size,
        total: lake_size,
    })
}

/// Return the recursive size of a directory, or zero when missing.
pub fn dir_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(files_with_sizes(path).map(|(_, len)| len).sum())
}

/// Iterate regular files under `root` with their sizes, without following
/// symlinks.
///
/// Entries that vanish or cannot be read mid-walk are skipped rather than
/// aborting the report: a concurrent `lake build` routinely creates and
/// deletes temporary files under `.lake` while it runs.
fn files_with_sizes(root: &Path) -> impl Iterator<Item = (DirEntry, u64)> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| {
            let len = entry.metadata().ok()?.len();
            Some((entry, len))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn named(name: &str) -> Project {
        Project {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            tags: vec!["active".to_string()],
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn unknown_size_selector_fails_loudly() {
        let projects = vec![named("demo")];
        let error = select_sized(&projects, Some("nope"), None).unwrap_err();
        assert!(error.to_string().contains("project not found: nope"));
        assert_eq!(
            select_sized(&projects, Some("demo"), None).unwrap().len(),
            1
        );
    }

    #[test]
    fn size_without_selector_uses_tag_filter() {
        let projects = vec![named("demo")];
        assert_eq!(select_sized(&projects, None, None).unwrap().len(), 1);
        assert_eq!(
            select_sized(&projects, None, Some("active")).unwrap().len(),
            1
        );
        assert!(
            select_sized(&projects, None, Some("other"))
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn dir_size_skips_unreadable_subtrees() {
        use std::os::unix::fs::PermissionsExt;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-size-locked-{nonce}"));
        fs::create_dir_all(root.join("locked")).unwrap();
        fs::write(root.join("visible"), b"abcd").unwrap();
        fs::write(root.join("locked/hidden"), b"zz").unwrap();
        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o000)).unwrap();

        let size = dir_size(&root);

        fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&root).unwrap();
        // Running as root bypasses the permission bits, so the locked file
        // may still be counted; either way the walk must not error.
        let size = size.expect("an unreadable subtree must not abort the walk");
        assert!(size == 4 || size == 6, "unexpected size {size}");
    }

    #[test]
    fn project_size_buckets_build_and_packages_in_one_pass() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-size-{nonce}"));
        fs::create_dir_all(root.join(".lake/build")).unwrap();
        fs::create_dir_all(root.join(".lake/packages/mathlib")).unwrap();
        fs::write(root.join(".lake/build/a"), b"aaaa").unwrap(); // 4
        fs::write(root.join(".lake/packages/mathlib/b"), b"bb").unwrap(); // 2
        fs::write(root.join(".lake/meta"), b"m").unwrap(); // 1

        let project = Project {
            name: "demo".to_string(),
            path: root.display().to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        };

        let size = project_size(&project).unwrap();
        assert_eq!(size.build, 4);
        assert_eq!(size.packages, 2);
        assert_eq!(size.lake, 7);
        assert_eq!(size.total, 7);

        fs::remove_dir_all(root).unwrap();
    }
}
