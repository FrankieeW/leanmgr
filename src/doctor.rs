//! Doctor diagnostics for Lean project cache health.

use crate::cli::DoctorArgs;
use crate::config::load_config;
use crate::output::{format_bytes, print_json};
use crate::project::Project;
use crate::size::{ProjectSize, project_size};
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
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

    Ok(())
}

/// Build doctor diagnostics from projects.
pub fn build_doctor_report(projects: &[Project], unused_days: u64) -> Result<DoctorReport> {
    let mut report = DoctorReport::default();
    let cutoff = unused_days
        .checked_mul(24 * 60 * 60)
        .and_then(|seconds| SystemTime::now().checked_sub(Duration::from_secs(seconds)))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for project in projects {
        let root = project.expanded_path();
        if !root.exists() {
            report.missing_paths.push(project.name.clone());
            continue;
        }
        if !root.join("lakefile.toml").exists() && !root.join("lakefile.lean").exists() {
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

        let toolchain = read_toolchain(project).unwrap_or_else(|| "missing".to_string());
        *report.toolchains.entry(toolchain).or_insert(0) += 1;
    }

    report
        .unused_projects
        .sort_by_key(|project| std::cmp::Reverse(project.total));
    Ok(report)
}

fn is_unused(path: &std::path::Path, cutoff: SystemTime) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let modified = fs::metadata(path)?.modified().unwrap_or(SystemTime::now());
    Ok(modified < cutoff)
}

fn read_toolchain(project: &Project) -> Option<String> {
    let content = fs::read_to_string(project.expanded_path().join("lean-toolchain")).ok()?;
    let value = content.lines().next()?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}
