//! Disk usage accounting for `.lake` cache trees.

use crate::cli::SizeArgs;
use crate::config::load_config;
use crate::output::{format_bytes, print_json, print_table};
use crate::project::{Project, filter_by_tag, matches_project};
use anyhow::{Result, bail};
use serde::Serialize;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;
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
    /// Newest modification time of anything under `.lake`; `None` when
    /// there is no `.lake`. Feeds staleness checks, not JSON output.
    #[serde(skip)]
    pub latest_modified: Option<SystemTime>,
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
    for size in measure_projects(&selected) {
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

/// Measure several projects at once, returning sizes in input order.
///
/// Walking `.lake` trees is dominated by filesystem metadata calls, so a
/// handful of walks in flight finish much sooner than one after another.
pub fn measure_projects(projects: &[&Project]) -> Vec<ProjectSize> {
    const MAX_WORKERS: usize = 8;
    let workers = std::thread::available_parallelism()
        .map_or(1, NonZeroUsize::get)
        .min(MAX_WORKERS)
        .min(projects.len());
    if workers <= 1 {
        return projects
            .iter()
            .map(|project| project_size(project))
            .collect();
    }

    let next = AtomicUsize::new(0);
    let mut measured: Vec<(usize, ProjectSize)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(project) = projects.get(index) else {
                            break done;
                        };
                        done.push((index, project_size(project)));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    measured.sort_by_key(|(index, _)| *index);
    measured.into_iter().map(|(_, size)| size).collect()
}

/// Compute `.lake`-related size for one project.
///
/// Walks `.lake` once: file bytes are bucketed by subtree (`build` and
/// `packages` are subtrees of `.lake`), and the newest modification time
/// of any entry is recorded, so callers needing both size and staleness
/// never walk the tree twice. Entries that vanish or cannot be read
/// mid-walk (a concurrent `lake build`) are skipped.
pub fn project_size(project: &Project) -> ProjectSize {
    let lake = project.expanded_path().join(".lake");
    let build = lake.join("build");
    let packages = lake.join("packages");
    let mut size = ProjectSize {
        project: project.name.clone(),
        ..ProjectSize::default()
    };
    if !lake.exists() {
        return size;
    }

    let entries = WalkDir::new(&lake)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok);
    for entry in entries {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if let Ok(modified) = metadata.modified() {
            size.latest_modified = size.latest_modified.max(Some(modified));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let len = metadata.len();
        size.lake += len;
        if entry.path().starts_with(&build) {
            size.build += len;
        } else if entry.path().starts_with(&packages) {
            size.packages += len;
        }
    }
    size.total = size.lake;
    size
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
    fn measure_projects_matches_sequential_sizes_in_input_order() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-measure-{nonce}"));
        let projects: Vec<Project> = (0..12)
            .map(|index| {
                let dir = root.join(format!("p{index}"));
                fs::create_dir_all(dir.join(".lake/build")).unwrap();
                fs::write(dir.join(".lake/build/blob"), vec![b'x'; index * 100]).unwrap();
                Project {
                    path: dir.display().to_string(),
                    ..named(&format!("p{index}"))
                }
            })
            .collect();
        let refs: Vec<&Project> = projects.iter().collect();

        let parallel = measure_projects(&refs);

        let names: Vec<&str> = parallel.iter().map(|size| size.project.as_str()).collect();
        let expected: Vec<String> = (0..12).map(|index| format!("p{index}")).collect();
        assert_eq!(names, expected);
        for (size, project) in parallel.iter().zip(&projects) {
            let sequential = project_size(project);
            assert_eq!(size.total, sequential.total);
            assert_eq!(size.build, sequential.build);
        }
        assert!(measure_projects(&[]).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn project_size_records_the_newest_modification() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-size-mtime-{nonce}"));
        fs::create_dir_all(root.join(".lake/build")).unwrap();
        let newest = SystemTime::now() + std::time::Duration::from_secs(7200);
        fs::write(root.join(".lake/build/fresh"), b"x").unwrap();
        fs::File::options()
            .write(true)
            .open(root.join(".lake/build/fresh"))
            .unwrap()
            .set_modified(newest)
            .unwrap();
        let project = Project {
            path: root.display().to_string(),
            ..named("mtime")
        };

        assert_eq!(project_size(&project).latest_modified, Some(newest));
        let no_lake = Project {
            path: root.join("absent").display().to_string(),
            ..named("absent")
        };
        assert_eq!(project_size(&no_lake).latest_modified, None);
        fs::remove_dir_all(&root).unwrap();
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

        let size = project_size(&project);
        assert_eq!(size.build, 4);
        assert_eq!(size.packages, 2);
        assert_eq!(size.lake, 7);
        assert_eq!(size.total, 7);

        fs::remove_dir_all(root).unwrap();
    }
}
