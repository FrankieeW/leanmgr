//! Restore orchestration around `lake exe cache get`.

use crate::cli::RestoreArgs;
use crate::process::run_in;
use crate::project::{Project, select_projects};
use anyhow::{Result, bail};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use std::process::Output;

/// Run the restore command.
pub fn restore_command(args: RestoreArgs) -> Result<()> {
    let projects = select_projects(args.project.as_deref(), args.tag.as_deref(), args.all)?;
    let bar = ProgressBar::new(projects.len() as u64);
    bar.set_style(
        ProgressStyle::with_template("{wide_bar} {pos}/{len} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_bar()),
    );

    let mut failures = Vec::new();
    for project in projects {
        bar.set_message(project.name.clone());
        if let Err(failure) = run_restore_for(&project, lake_runner) {
            failures.push(failure);
        }
        bar.inc(1);
    }
    bar.finish_and_clear();

    if failures.is_empty() {
        println!("Restore completed.");
        return Ok(());
    }

    for (project, error) in &failures {
        println!("Restore failed for {project}: {error}");
    }
    bail!("restore failed for {} project(s)", failures.len())
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
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitStatus, Output};

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

    fn exit_status(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: exit_status(code),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
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
