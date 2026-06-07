//! Git worktree audit commands.

use crate::cli::{WorktreeArgs, WorktreePruneArgs};
use crate::config::load_config;
use crate::output::{print_json, print_table};
use crate::process::run_in;
use crate::project::Project;
use anyhow::{Result, bail};
use serde::Serialize;
use std::io::{self, Write};
use std::path::Path;
use std::process::Output;

/// Worktree row.
#[derive(Clone, Debug, Serialize)]
pub struct WorktreeInfo {
    /// Project name.
    pub project: String,
    /// Worktree path.
    pub path: String,
    /// Branch if known.
    pub branch: Option<String>,
    /// Whether the worktree is prunable according to Git.
    pub prunable: bool,
}

/// List worktrees for indexed projects.
pub fn list_worktrees(args: WorktreeArgs) -> Result<()> {
    let infos = collect_worktrees()?;
    if args.json {
        return print_json(&infos);
    }
    let rows: Vec<Vec<String>> = infos
        .iter()
        .map(|info| {
            vec![
                info.project.clone(),
                info.path.clone(),
                info.branch.clone().unwrap_or_default(),
                info.prunable.to_string(),
            ]
        })
        .collect();
    print_table(&["PROJECT", "WORKTREE PATH", "BRANCH", "PRUNABLE"], &rows);
    Ok(())
}

/// Report broken/prunable worktrees.
pub fn doctor_worktrees(args: WorktreeArgs) -> Result<()> {
    let infos: Vec<WorktreeInfo> = collect_worktrees()?
        .into_iter()
        .filter(|info| info.prunable)
        .collect();
    if args.json {
        return print_json(&infos);
    }
    println!("Broken worktrees: {}", infos.len());
    for info in infos {
        println!("  {} {}", info.project, info.path);
    }
    Ok(())
}

/// Prune Git worktrees for every indexed project by delegating to Git.
pub fn prune_worktrees(args: WorktreePruneArgs) -> Result<()> {
    let config = load_config()?;

    if args.dry_run {
        let report = prune_projects(&config.projects, true, run_git_worktree_prune)?;
        print_prune_report(&report);
        return report.into_result();
    }

    if !args.force && !confirm_prune()? {
        println!("Aborted.");
        return Ok(());
    }

    let report = prune_projects(&config.projects, false, run_git_worktree_prune)?;
    print_prune_report(&report);
    report.into_result()
}

#[derive(Debug, Default)]
struct PruneReport {
    dry_run_lines: Vec<(String, String)>,
    failures: Vec<(String, String)>,
}

impl PruneReport {
    fn into_result(self) -> Result<()> {
        if self.failures.is_empty() {
            return Ok(());
        }
        bail!(
            "git worktree prune failed for {} project(s)",
            self.failures.len()
        )
    }
}

fn prune_projects<F>(projects: &[Project], dry_run: bool, mut runner: F) -> Result<PruneReport>
where
    F: FnMut(&Path, bool) -> Result<Output>,
{
    let mut report = PruneReport::default();
    for project in projects {
        match runner(&project.expanded_path(), dry_run) {
            Ok(output) if output.status.success() => {
                if dry_run {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        report
                            .dry_run_lines
                            .push((project.name.clone(), line.to_string()));
                    }
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                report.failures.push((project.name.clone(), stderr));
            }
            Err(error) => report
                .failures
                .push((project.name.clone(), error.to_string())),
        }
    }
    Ok(report)
}

fn run_git_worktree_prune(path: &Path, dry_run: bool) -> Result<Output> {
    let args = if dry_run {
        &["worktree", "prune", "--dry-run", "--verbose"][..]
    } else {
        &["worktree", "prune"][..]
    };
    run_in(path, "git", args)
}

fn print_prune_report(report: &PruneReport) {
    for (project, line) in &report.dry_run_lines {
        println!("{project}  {line}");
    }
    for (project, error) in &report.failures {
        println!("Prune failed for {project}: {error}");
    }
}

fn collect_worktrees() -> Result<Vec<WorktreeInfo>> {
    let config = load_config()?;
    let mut infos = Vec::new();
    for project in config.projects {
        let output = run_in(
            &project.expanded_path(),
            "git",
            &["worktree", "list", "--porcelain"],
        );
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        infos.extend(parse_worktrees(&project.name, &text));
    }
    Ok(infos)
}

/// Parse `git worktree list --porcelain` output.
pub fn parse_worktrees(project: &str, text: &str) -> Vec<WorktreeInfo> {
    let mut infos = Vec::new();
    let mut path: Option<String> = None;
    let mut branch: Option<String> = None;
    let mut prunable = false;

    for line in text.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if let Some(path) = path.take() {
                infos.push(WorktreeInfo {
                    project: project.to_string(),
                    path,
                    branch: branch.take(),
                    prunable,
                });
            }
            prunable = false;
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(value.trim_start_matches("refs/heads/").to_string());
        } else if line == "prunable" || line.starts_with("prunable ") {
            prunable = true;
        }
    }

    infos
}

fn confirm_prune() -> Result<bool> {
    print!("Run git worktree prune? [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{ExitStatus, Output};

    #[test]
    fn parses_porcelain_worktrees() {
        let input = "\
worktree /repo
HEAD abc123
branch refs/heads/main

worktree /repo-feature
HEAD def456
branch refs/heads/feature
prunable gitdir file points to non-existent location

";
        let infos = parse_worktrees("demo", input);
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].path, "/repo");
        assert_eq!(infos[0].branch.as_deref(), Some("main"));
        assert!(!infos[0].prunable);
        assert_eq!(infos[1].path, "/repo-feature");
        assert_eq!(infos[1].branch.as_deref(), Some("feature"));
        assert!(infos[1].prunable);
    }

    #[test]
    fn dry_run_prune_reports_failures() {
        let projects = vec![project("ok"), project("bad")];
        let report = prune_projects(&projects, true, |path, dry_run| {
            assert!(dry_run);
            if path.ends_with("bad") {
                Ok(output(1, "", "not a git repository"))
            } else {
                Ok(output(0, "would prune /tmp/stale\n", ""))
            }
        })
        .unwrap();

        assert_eq!(
            report.dry_run_lines,
            vec![("ok".to_string(), "would prune /tmp/stale".to_string())]
        );
        assert_eq!(
            report.failures,
            vec![("bad".to_string(), "not a git repository".to_string())]
        );
        assert!(report.into_result().is_err());
    }

    fn project(name: &str) -> Project {
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

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }

    #[cfg(windows)]
    fn exit_status(code: i32) -> ExitStatus {
        use std::os::windows::process::ExitStatusExt;
        ExitStatus::from_raw(code as u32)
    }
}
