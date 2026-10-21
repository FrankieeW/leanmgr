//! Session entry points. The CLI dispatcher in `lib.rs` calls
//! `interact_command`; that picks between the keyboard session and
//! the pipe-mode session depending on whether stdin/stdout are a TTY.

use super::actions::{
    print_clean_dry_run, print_doctor_summary, print_gc_dry_run, print_project_list,
    print_restore_command,
};
use super::dashboard::{print_dashboard, print_menu};
use super::helpers::prompt;
use super::keyboard::run_keyboard_session;
use crate::cli::InteractArgs;
use crate::config::load_config;
use crate::doctor::build_doctor_report;
use crate::project::{Project, filter_by_tag};
use anyhow::Result;
use std::io::{BufRead, IsTerminal, Write};

/// Start the interactive assistant. This is the public entry point
/// the CLI dispatcher calls.
pub fn interact_command(args: InteractArgs) -> Result<()> {
    let config = load_config()?;
    let projects: Vec<Project> = filter_by_tag(&config.projects, args.tag.as_deref())
        .into_iter()
        .cloned()
        .collect();
    if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return run_keyboard_session(&projects, args.unused_days, args.tag.as_deref());
    }
    let input = std::io::stdin();
    let mut output = std::io::stdout();
    run_session(
        &projects,
        args.unused_days,
        args.tag.as_deref(),
        input.lock(),
        &mut output,
    )
}

/// Pipe-mode session: print the dashboard, then loop on a numbered
/// menu. Used when stdin/stdout is not a TTY (CI, scripts, or `lm < tty
/// </dev/null`).
pub fn run_session<R: BufRead, W: Write>(
    projects: &[Project],
    unused_days: u64,
    tag: Option<&str>,
    mut input: R,
    output: &mut W,
) -> Result<()> {
    if projects.is_empty() {
        writeln!(output, "No indexed projects in this scope.")?;
        writeln!(
            output,
            "Add projects with `leanmgr add <path>` or `leanmgr scan <root>`."
        )?;
        return Ok(());
    }

    // Building the report walks every `.lake`, so it is computed once and
    // rebuilt only when the user asks for a refresh.
    let mut doctor = build_doctor_report(projects, unused_days)?;
    loop {
        print_dashboard(projects, &doctor, unused_days, output)?;
        print_menu(output)?;
        let Some(choice) = prompt("Choose an action", &mut input, output)? else {
            break;
        };

        let result = match choice.trim() {
            "1" => print_project_list(projects, output),
            "2" => print_doctor_summary(&doctor, unused_days, output),
            "3" => print_gc_dry_run(projects, unused_days, tag, output),
            "4" => print_clean_dry_run(projects, &mut input, output),
            "5" => print_restore_command(projects, &mut input, output),
            "6" => {
                doctor = build_doctor_report(projects, unused_days)?;
                continue;
            }
            "q" | "Q" | "quit" | "exit" => break,
            other => {
                writeln!(output, "Unknown action: {other}")?;
                Ok(())
            }
        };
        // A bad answer inside an action (unknown project, mistyped level)
        // is reported and the menu shown again instead of ending the session.
        if let Err(error) = result {
            writeln!(output, "Error: {error:#}")?;
        }

        if prompt("Press Enter to continue", &mut input, output)?.is_none() {
            break;
        }
    }

    writeln!(output, "Done.")?;
    Ok(())
}
