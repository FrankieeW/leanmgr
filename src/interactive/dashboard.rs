//! Dashboard rendering for both the pipe-mode and the keyboard-mode
//! interactive sessions. `print_dashboard` is the headline view; the
//! action list is `print_menu`. They are split so callers can suppress
//! the menu (the keyboard session draws its own menu in `keyboard.rs`).

use super::helpers::{print_section_to, print_table_to};
use super::summary::summarize_fleet;
use crate::doctor::DoctorReport;
use crate::output::format_bytes;
use crate::project::Project;
use anyhow::Result;
use std::io::Write;

/// Headline fleet summary for the pipe-mode session: a labelled table
/// with project counts, reclaimable space, the unused threshold, and
/// the largest project.
pub(super) fn print_dashboard<W: Write>(
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
    output: &mut W,
) -> Result<()> {
    let summary = summarize_fleet(projects);
    print_section_to(output, "LeanMgr assistant")?;
    let largest = doctor
        .largest_project
        .as_ref()
        .map(|project| format!("{} ({})", project.project, format_bytes(project.total)))
        .unwrap_or_else(|| "none".to_string());
    let tags = if summary.tags.is_empty() {
        "none".to_string()
    } else {
        summary
            .tags
            .iter()
            .map(|(tag, count)| format!("{tag}:{count}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let rows = vec![
        vec!["Projects".to_string(), summary.project_count.to_string()],
        vec![
            "Cached size coverage".to_string(),
            format!("{}/{}", summary.measured_count, summary.project_count),
        ],
        vec![
            "Cached total".to_string(),
            format_bytes(summary.cached_total),
        ],
        vec![
            "Potential reclaim".to_string(),
            format_bytes(doctor.potential_hard_reclaim),
        ],
        vec![
            "Unused threshold".to_string(),
            format!("{unused_days} days"),
        ],
        vec![
            "Unused projects".to_string(),
            doctor.unused_projects.len().to_string(),
        ],
        vec![
            "Missing paths".to_string(),
            doctor.missing_paths.len().to_string(),
        ],
        vec![
            "Missing Lake files".to_string(),
            doctor.missing_lake_files.len().to_string(),
        ],
        vec!["Largest project".to_string(), largest],
        vec!["Tags".to_string(), tags],
    ];
    print_table_to(output, &["ITEM", "VALUE"], &rows)?;
    Ok(())
}

/// Numbered action list shown after the dashboard. Keyboard input is
/// the action's number (`1`-`5`); the keyboard session draws its own
/// highlighted variant in `keyboard.rs`.
pub(super) fn print_menu<W: Write>(output: &mut W) -> Result<()> {
    print_section_to(output, "Actions")?;
    writeln!(output, "1  List projects")?;
    writeln!(output, "2  Show doctor details")?;
    writeln!(output, "3  Plan gc dry-run for unused projects")?;
    writeln!(output, "4  Plan clean dry-run for one project")?;
    writeln!(output, "5  Build restore command for one project")?;
    writeln!(output, "6  Refresh dashboard")?;
    writeln!(output, "q  Quit")?;
    Ok(())
}
