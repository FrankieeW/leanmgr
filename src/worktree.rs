//! Git worktree audit commands.

use crate::cli::{WorktreeArgs, WorktreePruneArgs};
use crate::config::load_config;
use crate::output::{print_json, print_table};
use crate::process::run_in;
use anyhow::{Result, bail};
use serde::Serialize;
use std::io::{self, Write};

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
        for project in &config.projects {
            let Ok(output) = run_in(
                &project.expanded_path(),
                "git",
                &["worktree", "prune", "--dry-run", "--verbose"],
            ) else {
                continue;
            };
            if !output.status.success() {
                continue;
            }
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                println!("{}  {line}", project.name);
            }
        }
        return Ok(());
    }

    if !args.force && !confirm_prune()? {
        println!("Aborted.");
        return Ok(());
    }

    let mut failures = Vec::new();
    for project in &config.projects {
        match run_in(&project.expanded_path(), "git", &["worktree", "prune"]) {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                failures.push((project.name.clone(), stderr));
            }
            Err(error) => failures.push((project.name.clone(), error.to_string())),
        }
    }

    if !failures.is_empty() {
        for (project, error) in &failures {
            println!("Prune failed for {project}: {error}");
        }
        bail!(
            "git worktree prune failed for {} project(s)",
            failures.len()
        );
    }
    Ok(())
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
}
