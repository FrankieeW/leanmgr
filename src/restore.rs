//! Restore orchestration around `lake exe cache get`.

use crate::cli::RestoreArgs;
use crate::lake::requires_mathlib;
use crate::output::print_table;
use crate::process::run_in;
use crate::project::{Project, select_projects};
use anyhow::{Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::process::Output;

/// The command restore runs for projects that can download artifacts.
const CACHE_GET: &str = "lake exe cache get";

/// Skip reason for a project whose directory no longer exists.
const PATH_MISSING: &str = "path missing";

/// What restore does for one project.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RestoreStep {
    /// Run `lake exe cache get` in the project directory.
    CacheGet,
    /// Leave the project alone, for the given reason.
    Skip(&'static str),
}

/// Decide how to restore one project.
///
/// `lake exe cache get` is Mathlib's artifact downloader, so it only exists
/// for Mathlib and projects that depend on it. Other projects are skipped
/// with a pointer to `lake build` instead of failing with "unknown
/// executable cache".
pub(crate) fn plan_restore(project: &Project) -> RestoreStep {
    let root = project.expanded_path();
    if !root.is_dir() {
        return RestoreStep::Skip(PATH_MISSING);
    }
    if !requires_mathlib(&root) {
        return RestoreStep::Skip("no Mathlib dependency; `lake build` rebuilds it");
    }
    RestoreStep::CacheGet
}

/// Run the restore command.
pub fn restore_command(args: RestoreArgs) -> Result<()> {
    let projects = select_projects(args.project.as_deref(), args.tag.as_deref(), args.all)?;
    let plan: Vec<(Project, RestoreStep)> = projects
        .into_iter()
        .map(|project| {
            let step = plan_restore(&project);
            (project, step)
        })
        .collect();

    if args.dry_run {
        print_restore_plan(&plan);
        return Ok(());
    }
    // Restoring a project the user named explicitly cannot succeed when
    // its directory is gone; that is an error, not a quiet skip.
    if args.project.is_some()
        && let Some((project, _)) = plan
            .iter()
            .find(|(_, step)| *step == RestoreStep::Skip(PATH_MISSING))
    {
        bail!(
            "cannot restore {}: {} no longer exists",
            project.name,
            project.path
        );
    }

    for (project, step) in &plan {
        if let RestoreStep::Skip(reason) = step {
            println!("Skipped {}: {reason}", project.name);
        }
    }
    let runnable: Vec<&Project> = plan
        .iter()
        .filter(|(_, step)| *step == RestoreStep::CacheGet)
        .map(|(project, _)| project)
        .collect();
    if runnable.is_empty() {
        println!("Nothing to restore.");
        return Ok(());
    }

    let failures = run_with_progress(&runnable);
    let restored = runnable.len() - failures.len();
    println!("Restored {restored} project(s).");
    if failures.is_empty() {
        return Ok(());
    }
    for (project, error) in &failures {
        println!("Restore failed for {project}: {error}");
    }
    bail!("restore failed for {} project(s)", failures.len())
}

fn print_restore_plan(plan: &[(Project, RestoreStep)]) {
    let rows: Vec<Vec<String>> = plan
        .iter()
        .map(|(project, step)| {
            let action = match step {
                RestoreStep::CacheGet => CACHE_GET.to_string(),
                RestoreStep::Skip(reason) => format!("skip: {reason}"),
            };
            vec![project.name.clone(), action]
        })
        .collect();
    print_table(&["PROJECT", "ACTION"], &rows);
}

/// Run `lake exe cache get` for each project behind a progress bar and
/// return the failures.
fn run_with_progress(projects: &[&Project]) -> Vec<(String, String)> {
    let bar = ProgressBar::new(projects.len() as u64);
    bar.set_style(
        ProgressStyle::with_template("{wide_bar} {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );
    let mut failures = Vec::new();
    for project in projects {
        bar.set_message(project.name.clone());
        if let Err(failure) = run_restore_for(project, lake_runner) {
            failures.push(failure);
        }
        bar.inc(1);
    }
    bar.finish_and_clear();
    failures
}

/// Execute `lake exe cache get` in the project's directory via the
/// supplied `runner`. Returns the project's name plus an error string on
/// failure, or `Ok(())` on success. Separated from `restore_command` so
/// the failure-handling branch is unit-testable without spawning `lake`.
///
/// `F` is a generic so callers can pass a plain closure; production uses
/// the `lake_runner` shim and tests inject a fake.
pub(crate) fn run_restore_for<F>(
    project: &Project,
    runner: F,
) -> std::result::Result<(), (String, String)>
where
    F: Fn(&Path, &[&str]) -> Result<Output>,
{
    let output = runner(&project.expanded_path(), &["exe", "cache", "get"]);
    match output {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err((project.name.clone(), stderr))
        }
        Err(error) => Err((project.name.clone(), error.to_string())),
    }
}

/// Production runner: shells out to `lake`.
fn lake_runner(dir: &Path, args: &[&str]) -> Result<Output> {
    run_in(dir, "lake", args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::process::{ExitStatus, Output};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("leanmgr-restore-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project_at(root: &Path) -> Project {
        Project {
            path: root.display().to_string(),
            ..project("demo")
        }
    }

    #[test]
    fn plan_runs_cache_get_for_mathlib_dependents() {
        let root = tmp("plan_mathlib");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": "1.1.0", "packages": [{"type": "git", "name": "mathlib"}]}"#,
        )
        .unwrap();
        assert_eq!(plan_restore(&project_at(&root)), RestoreStep::CacheGet);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_skips_projects_without_mathlib() {
        let root = tmp("plan_plain");
        fs::write(root.join("lakefile.toml"), b"name = \"demo\"\n").unwrap();
        assert!(matches!(
            plan_restore(&project_at(&root)),
            RestoreStep::Skip(reason) if reason.contains("no Mathlib")
        ));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn plan_skips_missing_paths() {
        let project = project_at(Path::new("/nonexistent/leanmgr-restore-gone"));
        assert_eq!(plan_restore(&project), RestoreStep::Skip(PATH_MISSING));
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

    #[test]
    fn ok_runner_records_no_failure() {
        let project = project("ok");
        let runner = |_dir: &Path, _args: &[&str]| Ok(output(0, "done", ""));
        let result = run_restore_for(&project, runner);
        assert!(result.is_ok());
    }

    #[test]
    fn failing_runner_records_stderr() {
        let project = project("bad");
        let runner = |_dir: &Path, _args: &[&str]| Ok(output(1, "", "lake: build failed"));
        let result = run_restore_for(&project, runner);
        let (name, message) = result.expect_err("must fail");
        assert_eq!(name, "bad");
        assert_eq!(message, "lake: build failed");
    }

    #[test]
    fn spawn_error_records_error_string() {
        let project = project("missing");
        let runner = |_dir: &Path, _args: &[&str]| -> Result<Output> {
            Err(anyhow::anyhow!("failed to run lake in /tmp/missing"))
        };
        let result = run_restore_for(&project, runner);
        let (name, message) = result.expect_err("must fail");
        assert_eq!(name, "missing");
        assert!(message.contains("failed to run lake"));
    }
}
