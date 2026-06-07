//! Restore orchestration around `lake exe cache get`.

use crate::cli::RestoreArgs;
use crate::process::run_in;
use crate::project::select_projects;
use anyhow::{Result, bail};
use indicatif::{ProgressBar, ProgressStyle};

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
        let output = run_in(&project.expanded_path(), "lake", &["exe", "cache", "get"]);
        match output {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                failures.push((project.name, stderr));
            }
            Err(error) => failures.push((project.name, error.to_string())),
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
