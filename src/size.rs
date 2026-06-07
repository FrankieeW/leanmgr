//! Disk usage accounting for `.lake` cache trees.

use crate::cli::SizeArgs;
use crate::config::load_config;
use crate::output::{format_bytes, print_json, print_table};
use crate::project::{Project, filter_by_tag, matches_project};
use anyhow::Result;
use serde::Serialize;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

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
    let selected: Vec<&Project> = if let Some(selector) = args.project.as_deref() {
        config
            .projects
            .iter()
            .filter(|project| matches_project(project, selector))
            .collect()
    } else {
        filter_by_tag(&config.projects, args.tag.as_deref())
    };

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
        for entry in WalkDir::new(&lake).follow_links(false) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let len = fs::metadata(entry.path())?.len();
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
    let mut total = 0;
    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total += fs::metadata(entry.path())?.len();
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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
