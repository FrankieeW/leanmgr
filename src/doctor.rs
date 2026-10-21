//! Doctor diagnostics for Lean project cache health.

use crate::cli::DoctorArgs;
use crate::config::load_config;
use crate::lake::{has_lakefile, read_toolchain};
use crate::output::{format_bytes, print_json};
use crate::project::Project;
use crate::shared_cache::{CacheDir, SharedCaches, collect_shared_caches};
use crate::size::{ProjectSize, measure_projects};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// Doctor report.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DoctorReport {
    /// Largest project by `.lake` size.
    pub largest_project: Option<ProjectSize>,
    /// Projects whose configured path no longer exists.
    pub missing_paths: Vec<String>,
    /// Projects missing a Lake file.
    pub missing_lake_files: Vec<String>,
    /// Projects unused by the heuristic threshold.
    pub unused_projects: Vec<ProjectSize>,
    /// Toolchain to project count.
    pub toolchains: BTreeMap<String, usize>,
    /// Potential reclaim from deleting all `.lake` directories.
    pub potential_hard_reclaim: u64,
}

/// `doctor --json` output: every report field plus the shared caches.
#[derive(Serialize)]
struct DoctorOutput<'a> {
    #[serde(flatten)]
    report: &'a DoctorReport,
    shared_caches: &'a SharedCaches,
}

/// Run the doctor command.
pub fn doctor_command(args: DoctorArgs) -> Result<()> {
    let config = load_config()?;
    // The shared caches are independent of the projects; measure both at once.
    let (report, shared) = std::thread::scope(|scope| {
        let shared = scope.spawn(collect_shared_caches);
        let report = build_doctor_report(&config.projects, args.unused_days);
        let shared = shared
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (report, shared)
    });
    let report = report?;

    if args.json {
        return print_json(&DoctorOutput {
            report: &report,
            shared_caches: &shared,
        });
    }

    print_doctor_report(&report, args.unused_days);
    print_shared_caches(&shared);
    for hint in doctor_hints(&report, &shared, args.unused_days) {
        println!("Hint: {hint}");
    }
    Ok(())
}

fn print_doctor_report(report: &DoctorReport, unused_days: u64) {
    if let Some(project) = &report.largest_project {
        println!(
            "Largest project: {} {}",
            project.project,
            format_bytes(project.total)
        );
    } else {
        println!("Largest project: none");
    }

    println!(
        "Potential reclaim: {}",
        format_bytes(report.potential_hard_reclaim)
    );

    if !report.unused_projects.is_empty() {
        println!("Unused > {unused_days} days:");
        for project in &report.unused_projects {
            println!("  {} {}", project.project, format_bytes(project.total));
        }
    }

    if !report.missing_paths.is_empty() {
        println!("Missing paths:");
        for item in &report.missing_paths {
            println!("  {item}");
        }
    }

    if !report.missing_lake_files.is_empty() {
        println!("Missing Lake files:");
        for item in &report.missing_lake_files {
            println!("  {item}");
        }
    }

    if !report.toolchains.is_empty() {
        println!("Toolchains:");
        for (toolchain, count) in &report.toolchains {
            println!("  {toolchain} {count}");
        }
    }
}

/// Print the machine-wide caches that live outside every project.
fn print_shared_caches(shared: &SharedCaches) {
    let dirs: Vec<(&str, &CacheDir)> = [
        ("Mathlib downloads", shared.mathlib_downloads.as_ref()),
        ("elan toolchains", shared.elan_toolchains.as_ref()),
    ]
    .into_iter()
    .filter_map(|(label, dir)| dir.map(|dir| (label, dir)))
    .collect();
    if dirs.is_empty() && shared.unused_toolchains.is_none() {
        return;
    }
    println!("Shared caches:");
    for (label, dir) in &dirs {
        println!(
            "  {label:<18} {:>10}  {}",
            format_bytes(dir.bytes),
            dir.path.display()
        );
    }
    println!(
        "  {:<18} {}",
        "Unused toolchains",
        unused_toolchains_cell(shared)
    );
}

fn unused_toolchains_cell(shared: &SharedCaches) -> String {
    match &shared.unused_toolchains {
        None => "unknown (`elan toolchain gc` unavailable)".to_string(),
        Some(unused) if unused.is_empty() => "none".to_string(),
        Some(unused) => {
            let names: Vec<&str> = unused
                .iter()
                .map(|toolchain| toolchain.toolchain.as_str())
                .collect();
            format!(
                "{} ({}): {}",
                unused.len(),
                format_bytes(shared.unused_toolchain_bytes()),
                names.join(", ")
            )
        }
    }
}

/// Next-step suggestions for the problems a report found.
fn doctor_hints(report: &DoctorReport, shared: &SharedCaches, unused_days: u64) -> Vec<String> {
    let mut hints = Vec::new();
    if !report.unused_projects.is_empty() {
        hints.push(format!(
            "`leanmgr gc --unused-days {unused_days} --dry-run` previews reclaiming the unused caches."
        ));
    }
    if !report.missing_paths.is_empty() {
        hints.push(
            "`leanmgr remove --missing` drops index entries whose path is gone (source is never touched)."
                .to_string(),
        );
    }
    if let Some(unused) = shared
        .unused_toolchains
        .as_ref()
        .filter(|unused| !unused.is_empty())
    {
        hints.push(format!(
            "`elan toolchain gc --delete` removes {} toolchain(s) no known project uses ({}).",
            unused.len(),
            format_bytes(shared.unused_toolchain_bytes())
        ));
    }
    hints
}

/// Build doctor diagnostics from projects.
///
/// Each existing project's `.lake` is walked once — size and newest
/// modification time come from the same pass — and the walks run
/// concurrently.
pub fn build_doctor_report(projects: &[Project], unused_days: u64) -> Result<DoctorReport> {
    let mut report = DoctorReport::default();
    let cutoff = unused_cutoff(unused_days);

    let mut present = Vec::new();
    for project in projects {
        let root = project.expanded_path();
        if !root.exists() {
            report.missing_paths.push(project.name.clone());
            continue;
        }
        if !has_lakefile(&root) {
            report.missing_lake_files.push(project.name.clone());
        }
        let toolchain = read_toolchain(&root).unwrap_or_else(|| "missing".to_string());
        *report.toolchains.entry(toolchain).or_insert(0) += 1;
        present.push(project);
    }

    for size in measure_projects(&present) {
        report.potential_hard_reclaim += size.total;
        if report
            .largest_project
            .as_ref()
            .is_none_or(|current| size.total > current.total)
        {
            report.largest_project = Some(size.clone());
        }
        // The newest mtime of any entry, not the `.lake` dir's own mtime:
        // incremental builds rewrite files without touching directories.
        if size.total > 0
            && size
                .latest_modified
                .is_some_and(|modified| modified < cutoff)
        {
            report.unused_projects.push(size);
        }
    }

    report
        .unused_projects
        .sort_by_key(|project| std::cmp::Reverse(project.total));
    Ok(report)
}

/// Cutoff time before which a `.lake` is considered unused.
pub(crate) fn unused_cutoff(unused_days: u64) -> SystemTime {
    unused_days
        .checked_mul(24 * 60 * 60)
        .and_then(|seconds| SystemTime::now().checked_sub(Duration::from_secs(seconds)))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Decide whether a cache whose newest entry was modified at `latest` is
/// unused.
///
/// `None` means there is no `.lake` at all, which is not a stale cache.
/// This is the single decision point for staleness: callers that already
/// know the newest mtime (from a size scan) do not have to walk the tree
/// a second time.
pub fn is_unused_mtime(latest: Option<SystemTime>, cutoff: SystemTime) -> bool {
    match latest {
        None => false,
        Some(latest) => latest < cutoff,
    }
}

/// Decide whether the `.lake` tree at `path` is unused.
///
/// This walks the tree. Callers that already know the newest mtime should
/// use [`is_unused_mtime`] instead so the cache is not walked twice.
pub fn is_unused(path: &std::path::Path, cutoff: SystemTime) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    // A directory's own mtime only updates when entries are added or removed,
    // not when existing files are rewritten. Incremental `lake build` modifies
    // files under `.lake/build/` without touching the dir mtime, which made
    // actively-used projects look stale. Walk the tree and use the maximum
    // mtime so any recent file activity is detected.
    let mut latest = SystemTime::UNIX_EPOCH;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if mtime > latest {
            latest = mtime;
            if latest >= cutoff {
                return Ok(false);
            }
        }
    }
    Ok(is_unused_mtime(Some(latest), cutoff))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn hints_follow_findings() {
        let mut report = DoctorReport::default();
        let shared = SharedCaches::default();
        assert!(doctor_hints(&report, &shared, 90).is_empty());

        report.missing_paths.push("gone".to_string());
        report.unused_projects.push(ProjectSize::default());
        let hints = doctor_hints(&report, &shared, 30);
        assert_eq!(hints.len(), 2);
        assert!(hints[0].contains("gc --unused-days 30 --dry-run"));
        assert!(hints[1].contains("remove --missing"));
    }

    #[test]
    fn unused_toolchains_produce_an_elan_hint() {
        use crate::shared_cache::UnusedToolchain;
        let report = DoctorReport::default();
        let mut shared = SharedCaches {
            unused_toolchains: Some(Vec::new()),
            ..SharedCaches::default()
        };
        assert!(doctor_hints(&report, &shared, 90).is_empty());
        assert_eq!(unused_toolchains_cell(&shared), "none");

        shared.unused_toolchains = Some(vec![UnusedToolchain {
            toolchain: "leanprover/lean4:v4.8.0".to_string(),
            bytes: Some(2048),
        }]);
        let hints = doctor_hints(&report, &shared, 90);
        assert_eq!(hints.len(), 1);
        assert!(hints[0].contains("elan toolchain gc --delete"));
        assert!(hints[0].contains("2.0 KiB"));
        assert_eq!(
            unused_toolchains_cell(&shared),
            "1 (2.0 KiB): leanprover/lean4:v4.8.0"
        );

        shared.unused_toolchains = None;
        assert!(unused_toolchains_cell(&shared).starts_with("unknown"));
    }

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }

    #[test]
    fn is_unused_compares_against_cutoff() {
        let dir = tmp("is_unused");
        fs::create_dir_all(&dir).unwrap();
        let future = SystemTime::now() + Duration::from_secs(3600);
        let past = SystemTime::now() - Duration::from_secs(3600);
        assert!(is_unused(&dir, future).unwrap());
        assert!(!is_unused(&dir, past).unwrap());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn report_flags_only_caches_without_recent_writes() {
        let root = tmp("doctor_unused");
        let make = |name: &str| {
            let dir = root.join(name);
            fs::create_dir_all(dir.join(".lake/build")).unwrap();
            fs::write(dir.join("lakefile.toml"), b"").unwrap();
            fs::write(dir.join(".lake/build/out.olean"), b"data").unwrap();
            Project {
                name: name.to_string(),
                path: dir.display().to_string(),
                tags: Vec::new(),
                description: None,
                added_at: None,
                last_seen_at: None,
                last_committed_at: None,
                size_cache: None,
            }
        };
        let stale = make("stale");
        let active = make("active");
        // An incremental build rewrites a file deep inside `.lake` without
        // touching any directory mtime; that alone must keep it in use.
        fs::File::options()
            .write(true)
            .open(active.expanded_path().join(".lake/build/out.olean"))
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(3600))
            .unwrap();

        let report = build_doctor_report(&[stale, active], 0).unwrap();

        let unused: Vec<&str> = report
            .unused_projects
            .iter()
            .map(|size| size.project.as_str())
            .collect();
        assert_eq!(unused, vec!["stale"]);
        assert_eq!(report.potential_hard_reclaim, 8);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn is_unused_walks_recursively_for_recent_writes() {
        // Create the dir, then a moment later create a file inside. The file's
        // mtime is at least as fresh as the dir's; the recursive walk must
        // observe it. Without recursion, the dir's mtime alone could be older
        // than a cutoff that the file's mtime exceeds, which is exactly the
        // case that broke incremental `lake build` detection in the field.
        let dir = tmp("is_unused_recursive");
        fs::create_dir_all(&dir).unwrap();
        let dir_mtime = fs::metadata(&dir).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(10));
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("build/fresh.olean"), b"data").unwrap();
        let file_mtime = fs::metadata(dir.join("build/fresh.olean"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(
            file_mtime > dir_mtime,
            "test setup: file mtime must exceed dir mtime"
        );

        // Cutoff set to dir_mtime must NOT consider the dir unused; the file
        // is fresher than that.
        assert!(
            !is_unused(&dir, dir_mtime).unwrap(),
            "a fresh file under .lake must keep the project in use even when \
             the dir mtime alone is at the cutoff"
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
