//! Git worktree audit commands.

use crate::cli::{WorktreeArgs, WorktreePruneArgs};
use crate::config::load_config;
use crate::output::{print_json, print_table};
use crate::process::run_in;
use crate::project::Project;
use crate::prompt::confirm;
use anyhow::{Result, bail};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
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

/// One Git repository to audit, reached through the first indexed project
/// that lives in it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Repository {
    /// Name of the first indexed project inside the repository.
    project: String,
    /// Directory the Git commands run in.
    dir: PathBuf,
}

/// An indexed project that cannot be audited with Git, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Skipped {
    project: String,
    reason: &'static str,
}

const PATH_MISSING: &str = "path missing";
const NOT_A_REPOSITORY: &str = "not a git repository";

/// List worktrees for indexed projects.
pub fn list_worktrees(args: WorktreeArgs) -> Result<()> {
    let config = load_config()?;
    let (infos, skipped) = collect_worktrees(&config.projects, &mut run_git);
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
    print_skipped(&skipped);
    Ok(())
}

/// Report broken/prunable worktrees.
pub fn doctor_worktrees(args: WorktreeArgs) -> Result<()> {
    let config = load_config()?;
    let (infos, skipped) = collect_worktrees(&config.projects, &mut run_git);
    let broken: Vec<WorktreeInfo> = infos.into_iter().filter(|info| info.prunable).collect();
    if args.json {
        return print_json(&broken);
    }
    println!("Broken worktrees: {}", broken.len());
    for info in &broken {
        println!("  {} {}", info.project, info.path);
    }
    print_skipped(&skipped);
    Ok(())
}

/// Prune stale worktree records in every indexed repository by delegating
/// to `git worktree prune`.
///
/// Without `--force`, Git's own dry run is shown first and confirmation is
/// requested only when there is something to prune.
pub fn prune_worktrees(args: WorktreePruneArgs) -> Result<()> {
    let config = load_config()?;
    let mut git = run_git;
    let (repos, skipped) = repositories(&config.projects, &mut git);

    if args.dry_run || !args.force {
        let preview = prune_repositories(&repos, true, &mut git);
        print_prune_report(&preview);
        print_skipped(&skipped);
        if args.dry_run {
            return preview.into_result();
        }
        if preview.dry_run_lines.is_empty() {
            println!("Nothing to prune.");
            return preview.into_result();
        }
        if !confirm("Prune these stale worktree records?")? {
            println!("Aborted.");
            return Ok(());
        }
    }

    let report = prune_repositories(&repos, false, &mut git);
    print_prune_report(&report);
    if args.force {
        print_skipped(&skipped);
    }
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
            "git worktree prune failed for {} repository(ies)",
            self.failures.len()
        )
    }
}

/// Group indexed projects by Git repository.
///
/// Projects that share a repository (a nested Lake package, or several
/// worktrees of one repo) are audited once, so the same worktree is not
/// listed or pruned repeatedly. Missing paths and directories outside Git
/// are reported as skipped instead of failing the whole command.
fn repositories<F>(projects: &[Project], git: &mut F) -> (Vec<Repository>, Vec<Skipped>)
where
    F: FnMut(&Path, &[&str]) -> Result<Output>,
{
    let mut seen = HashSet::new();
    let mut repos = Vec::new();
    let mut skipped = Vec::new();
    for project in projects {
        let dir = project.expanded_path();
        if !dir.is_dir() {
            skipped.push(Skipped {
                project: project.name.clone(),
                reason: PATH_MISSING,
            });
            continue;
        }
        let Some(common_dir) = git_common_dir(&dir, git) else {
            skipped.push(Skipped {
                project: project.name.clone(),
                reason: NOT_A_REPOSITORY,
            });
            continue;
        };
        if seen.insert(common_dir) {
            repos.push(Repository {
                project: project.name.clone(),
                dir,
            });
        }
    }
    (repos, skipped)
}

/// Return the canonical common Git directory of `dir` — the same for every
/// worktree of a repository — or `None` when `dir` is not inside Git.
fn git_common_dir<F>(dir: &Path, git: &mut F) -> Option<PathBuf>
where
    F: FnMut(&Path, &[&str]) -> Result<Output>,
{
    let output = git(dir, &["rev-parse", "--git-common-dir"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    // From a subdirectory Git prints a path relative to `dir`; `join` keeps
    // absolute paths unchanged.
    let path = dir.join(raw);
    Some(path.canonicalize().unwrap_or(path))
}

fn prune_repositories<F>(repos: &[Repository], dry_run: bool, git: &mut F) -> PruneReport
where
    F: FnMut(&Path, &[&str]) -> Result<Output>,
{
    let args: &[&str] = if dry_run {
        &["worktree", "prune", "--dry-run", "--verbose"]
    } else {
        &["worktree", "prune"]
    };
    let mut report = PruneReport::default();
    for repo in repos {
        match git(&repo.dir, args) {
            Ok(output) if output.status.success() => {
                if dry_run {
                    // `git worktree prune --verbose` reports on stderr.
                    let text = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    );
                    for line in text.lines().filter(|line| !line.trim().is_empty()) {
                        report
                            .dry_run_lines
                            .push((repo.project.clone(), line.to_string()));
                    }
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                report.failures.push((repo.project.clone(), stderr));
            }
            Err(error) => report
                .failures
                .push((repo.project.clone(), error.to_string())),
        }
    }
    report
}

fn run_git(dir: &Path, args: &[&str]) -> Result<Output> {
    run_in(dir, "git", args)
}

fn print_prune_report(report: &PruneReport) {
    for (project, line) in &report.dry_run_lines {
        println!("{project}  {line}");
    }
    for (project, error) in &report.failures {
        println!("Prune failed for {project}: {error}");
    }
}

/// Print one line per skip reason, naming the affected projects.
fn print_skipped(skipped: &[Skipped]) {
    for reason in [PATH_MISSING, NOT_A_REPOSITORY] {
        let names: Vec<&str> = skipped
            .iter()
            .filter(|skip| skip.reason == reason)
            .map(|skip| skip.project.as_str())
            .collect();
        if !names.is_empty() {
            println!("Skipped ({reason}): {}", names.join(", "));
        }
    }
}

fn collect_worktrees<F>(projects: &[Project], git: &mut F) -> (Vec<WorktreeInfo>, Vec<Skipped>)
where
    F: FnMut(&Path, &[&str]) -> Result<Output>,
{
    let (repos, skipped) = repositories(projects, git);
    let mut infos = Vec::new();
    for repo in repos {
        let Ok(output) = git(&repo.dir, &["worktree", "list", "--porcelain"]) else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        infos.extend(parse_worktrees(&repo.project, &text));
    }
    (infos, skipped)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::{ExitStatus, Output};
    use std::time::{SystemTime, UNIX_EPOCH};

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
    fn repositories_skip_missing_paths_and_non_git_dirs_and_dedupe() {
        let root = tmp("repositories");
        for name in ["main", "nested", "plain"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let projects = vec![
            project("main", &root.join("main")),
            project("nested", &root.join("nested")),
            project("plain", &root.join("plain")),
            project("gone", &root.join("gone")),
        ];
        // `main` and `nested` live in the same repository; `plain` is not
        // inside Git at all.
        let shared = root.join("main/.git").display().to_string();
        let mut git = |dir: &Path, args: &[&str]| {
            assert_eq!(args, ["rev-parse", "--git-common-dir"]);
            if dir.ends_with("plain") {
                Ok(output(128, "", "fatal: not a git repository"))
            } else {
                Ok(output(0, &format!("{shared}\n"), ""))
            }
        };

        let (repos, skipped) = repositories(&projects, &mut git);

        assert_eq!(repos.len(), 1);
        assert_eq!(repos[0].project, "main");
        assert_eq!(
            skipped,
            vec![
                Skipped {
                    project: "plain".to_string(),
                    reason: NOT_A_REPOSITORY,
                },
                Skipped {
                    project: "gone".to_string(),
                    reason: PATH_MISSING,
                },
            ]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn relative_common_dir_is_resolved_against_project_dir() {
        let root = tmp("relative_common_dir");
        fs::create_dir_all(root.join("repo/.git")).unwrap();
        fs::create_dir_all(root.join("repo/sub")).unwrap();
        let mut git = |_dir: &Path, _args: &[&str]| Ok(output(0, "../.git\n", ""));

        let common = git_common_dir(&root.join("repo/sub"), &mut git).unwrap();

        assert_eq!(common, root.join("repo/.git").canonicalize().unwrap());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn dry_run_prune_reads_git_verbose_output_from_stderr() {
        let repos = vec![
            Repository {
                project: "ok".to_string(),
                dir: PathBuf::from("/tmp/ok"),
            },
            Repository {
                project: "bad".to_string(),
                dir: PathBuf::from("/tmp/bad"),
            },
        ];
        let mut git = |dir: &Path, args: &[&str]| {
            assert_eq!(args, ["worktree", "prune", "--dry-run", "--verbose"]);
            if dir.ends_with("bad") {
                Ok(output(1, "", "fatal: something broke"))
            } else {
                Ok(output(
                    0,
                    "",
                    "Removing worktrees/stale: gitdir file points to non-existent location\n",
                ))
            }
        };

        let report = prune_repositories(&repos, true, &mut git);

        assert_eq!(
            report.dry_run_lines,
            vec![(
                "ok".to_string(),
                "Removing worktrees/stale: gitdir file points to non-existent location".to_string()
            )]
        );
        assert_eq!(
            report.failures,
            vec![("bad".to_string(), "fatal: something broke".to_string())]
        );
        assert!(report.into_result().is_err());
    }

    #[test]
    fn real_prune_does_not_collect_dry_run_lines() {
        let repos = vec![Repository {
            project: "ok".to_string(),
            dir: PathBuf::from("/tmp/ok"),
        }];
        let mut git = |_dir: &Path, args: &[&str]| {
            assert_eq!(args, ["worktree", "prune"]);
            Ok(output(0, "", ""))
        };
        let report = prune_repositories(&repos, false, &mut git);
        assert!(report.dry_run_lines.is_empty());
        assert!(report.into_result().is_ok());
    }

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-worktree-{name}-{nonce}"))
    }

    fn project(name: &str, path: &Path) -> Project {
        Project {
            name: name.to_string(),
            path: path.display().to_string(),
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
