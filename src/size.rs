//! Disk usage accounting for `.lake` cache trees.

use crate::cli::SizeArgs;
use crate::config::load_config;
use crate::output::{format_bytes, print_json, print_table};
use crate::project::{Project, filter_by_tag, matches_project};
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
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

/// Bucketed view of one project's `.lake` tree, from a single traversal.
///
/// `size` answers "how big" and "when was it last touched" together, so a
/// command that needs both does not have to walk the cache twice.
#[derive(Clone, Debug, Default)]
pub struct LakeScan {
    /// Size buckets for the project.
    pub size: ProjectSize,
    /// `.lake/packages/<pkg>/.lake/build` paths that exist, with the bytes
    /// under each. Targets for the `deps-build` cleanup level.
    pub package_builds: Vec<(PathBuf, u64)>,
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
        args.all,
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
    all: bool,
) -> Result<Vec<&'a Project>> {
    if all {
        // `--all` is the explicit "no filter" form. Clap rejects it in
        // combination with a selector or a tag, so it can never silently
        // lose against one of them.
        return Ok(projects.iter().collect());
    }
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
    scan_lake(project).size
}

/// Traverse a project's `.lake` once, bucketing bytes by subtree, tracking the
/// newest modification time, and recording the per-package build directories
/// that the `deps-build` cleanup level targets.
pub fn scan_lake(project: &Project) -> LakeScan {
    let lake = project.expanded_path().join(".lake");
    let build = lake.join("build");
    let packages = lake.join("packages");
    let mut scan = LakeScan {
        size: ProjectSize {
            project: project.name.clone(),
            ..ProjectSize::default()
        },
        package_builds: Vec::new(),
    };
    if !lake.exists() {
        return scan;
    }

    // Seed the per-package map with the build directories that exist, so an
    // empty one is still a valid cleanup target.
    let mut package_bytes: BTreeMap<PathBuf, u64> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(&packages) {
        for entry in entries.flatten() {
            // `file_type` does not follow symlinks, so a symlinked package
            // directory is skipped rather than treated as a cache to delete.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let build_path = entry.path().join(".lake").join("build");
            if build_path.exists() {
                package_bytes.insert(build_path, 0);
            }
        }
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
            scan.size.latest_modified = scan.size.latest_modified.max(Some(modified));
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let len = metadata.len();
        scan.size.lake += len;
        if entry.path().starts_with(&build) {
            scan.size.build += len;
        } else if entry.path().starts_with(&packages) {
            scan.size.packages += len;
            if let Some(build_path) = package_build_path(&packages, entry.path()) {
                *package_bytes.entry(build_path).or_insert(0) += len;
            }
        }
    }
    scan.size.total = scan.size.lake;
    scan.package_builds = package_bytes.into_iter().collect();
    scan
}

/// Return the `<packages>/<pkg>/.lake/build` path a file belongs to, if any.
fn package_build_path(packages: &Path, file: &Path) -> Option<PathBuf> {
    let relative = file.strip_prefix(packages).ok()?;
    let mut components = relative.components();
    let package = components.next()?;
    if components.next()?.as_os_str() != ".lake" {
        return None;
    }
    if components.next()?.as_os_str() != "build" {
        return None;
    }
    Some(packages.join(package).join(".lake").join("build"))
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
        let error = select_sized(&projects, Some("nope"), None, false).unwrap_err();
        assert!(error.to_string().contains("project not found: nope"));
        assert_eq!(
            select_sized(&projects, Some("demo"), None, false)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn size_without_selector_uses_tag_filter() {
        let projects = vec![named("demo")];
        assert_eq!(select_sized(&projects, None, None, false).unwrap().len(), 1);
        assert_eq!(
            select_sized(&projects, None, Some("active"), false)
                .unwrap()
                .len(),
            1
        );
        assert!(
            select_sized(&projects, None, Some("other"), false)
                .unwrap()
                .is_empty()
        );
    }

    /// `--all` is the explicit "no filter" form. It must actually be read,
    /// and it must not silently lose against a tag filter.
    #[test]
    fn size_all_selects_every_project() {
        let projects = vec![named("demo"), named("other")];
        assert_eq!(select_sized(&projects, None, None, true).unwrap().len(), 2);
    }

    /// `scan_lake` must report the per-package build directories that the
    /// `deps-build` cleanup level targets, in one traversal.
    #[test]
    fn scan_lake_reports_package_build_directories() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-scan-{nonce}"));
        fs::create_dir_all(root.join(".lake/packages/mathlib/.lake/build")).unwrap();
        fs::create_dir_all(root.join(".lake/packages/batteries/.lake/build")).unwrap();
        fs::write(root.join(".lake/packages/mathlib/.lake/build/a"), b"aaaa").unwrap();
        fs::write(root.join(".lake/packages/batteries/.lake/build/b"), b"bb").unwrap();

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

        let scan = scan_lake(&project);
        assert_eq!(scan.size.lake, 6);
        assert_eq!(scan.size.packages, 6);
        assert_eq!(scan.package_builds.len(), 2);

        let mathlib = root.join(".lake/packages/mathlib/.lake/build");
        let batteries = root.join(".lake/packages/batteries/.lake/build");
        assert!(scan.package_builds.contains(&(mathlib, 4)));
        assert!(scan.package_builds.contains(&(batteries, 2)));

        fs::remove_dir_all(root).unwrap();
    }

    /// An empty package build directory is still a cleanup target.
    #[test]
    fn scan_lake_keeps_empty_package_build_directories() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-scan-empty-{nonce}"));
        fs::create_dir_all(root.join(".lake/packages/empty/.lake/build")).unwrap();

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

        let scan = scan_lake(&project);
        assert_eq!(
            scan.package_builds,
            vec![(root.join(".lake/packages/empty/.lake/build"), 0)]
        );

        fs::remove_dir_all(root).unwrap();
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
