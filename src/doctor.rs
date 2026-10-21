//! Doctor diagnostics for Lean project cache health.

use crate::cli::DoctorArgs;
use crate::config::load_config;
use crate::lake::{has_lakefile, read_toolchain};
use crate::output::{format_bytes, print_json};
use crate::project::Project;
use crate::size::{ProjectSize, project_size};
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

/// Run the doctor command.
pub fn doctor_command(args: DoctorArgs) -> Result<()> {
    let config = load_config()?;
    let report = build_doctor_report(&config.projects, args.unused_days)?;

    if args.json {
        return print_json(&report);
    }

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
        println!("Unused > {} days:", args.unused_days);
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

    for hint in doctor_hints(&report, args.unused_days) {
        println!("Hint: {hint}");
    }

    Ok(())
}

/// Next-step suggestions for the problems a report found.
fn doctor_hints(report: &DoctorReport, unused_days: u64) -> Vec<String> {
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
    hints
}

/// Build doctor diagnostics from projects.
pub fn build_doctor_report(projects: &[Project], unused_days: u64) -> Result<DoctorReport> {
    let mut report = DoctorReport::default();
    let cutoff = unused_cutoff(unused_days);

    for project in projects {
        let root = project.expanded_path();
        if !root.exists() {
            report.missing_paths.push(project.name.clone());
            continue;
        }
        if !has_lakefile(&root) {
            report.missing_lake_files.push(project.name.clone());
        }

        let size = project_size(project)?;
        report.potential_hard_reclaim += size.total;
        if report
            .largest_project
            .as_ref()
            .is_none_or(|current| size.total > current.total)
        {
            report.largest_project = Some(size.clone());
        }

        if is_unused(&root.join(".lake"), cutoff)? && size.total > 0 {
            report.unused_projects.push(size);
        }

        let toolchain = read_toolchain(&root).unwrap_or_else(|| "missing".to_string());
        *report.toolchains.entry(toolchain).or_insert(0) += 1;
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

pub(crate) fn is_unused(path: &std::path::Path, cutoff: SystemTime) -> Result<bool> {
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
    Ok(latest < cutoff)
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
        assert!(doctor_hints(&report, 90).is_empty());

        report.missing_paths.push("gone".to_string());
        report.unused_projects.push(ProjectSize::default());
        let hints = doctor_hints(&report, 30);
        assert_eq!(hints.len(), 2);
        assert!(hints[0].contains("gc --unused-days 30 --dry-run"));
        assert!(hints[1].contains("remove --missing"));
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
