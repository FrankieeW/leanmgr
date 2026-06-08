//! Keyboard-mode interactive session. Renders a highlighted menu, reads
//! a single keypress at a time, and dispatches to the action renderers
//! in `actions.rs`.

use super::actions::{
    print_clean_dry_run, print_doctor_summary, print_gc_dry_run, print_project_list,
    print_restore_command,
};
use super::helpers::{truncate_display, write_menu_line, write_metric_line};
use super::summary::summarize_fleet;
use crate::doctor::DoctorReport;
use crate::output::format_bytes;
use crate::project::Project;
use anyhow::Result;
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType};
use std::io::Write;

/// Action the keyboard session should take next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MenuAction {
    Open(usize),
    Refresh,
    Quit,
    Redraw,
}

/// Menu rows for the keyboard dashboard. Order matches the action
/// indices used by `read_menu_action` and `run_menu_action`.
const KEYBOARD_ACTIONS: &[(&str, &str)] = &[
    ("List projects", "Browse indexed projects and cached sizes"),
    (
        "Doctor details",
        "Review missing paths, Lake files, and toolchains",
    ),
    ("GC dry-run", "Preview stale recoverable cache cleanup"),
    ("Clean dry-run", "Plan cleanup for one project"),
    ("Restore command", "Build a restore command for one project"),
];

/// Number of lines the keyboard menu occupies. Used to rewind the
/// cursor on each redraw so we don't need a fullscreen alternate
/// terminal — output stays in normal scrollback.
const KEYBOARD_MENU_LINES: u16 = 13;

/// Run the keyboard dashboard for the duration of one session.
pub(super) fn run_keyboard_session(projects: &[Project], unused_days: u64) -> Result<()> {
    if projects.is_empty() {
        println!("No indexed projects in this scope.");
        println!("Add projects with `leanmgr add <path>` or `leanmgr scan <root>`.");
        return Ok(());
    }

    let mut output = std::io::stdout();
    let mut selected = 0usize;
    let mut first_render = true;

    loop {
        let doctor = crate::doctor::build_doctor_report(projects, unused_days)?;
        render_keyboard_menu(
            projects,
            &doctor,
            unused_days,
            selected,
            first_render,
            &mut output,
        )?;
        first_render = false;

        crossterm::terminal::enable_raw_mode()?;
        let action = read_menu_action(&mut selected);
        let result = crossterm::terminal::disable_raw_mode();
        result?;
        let action = action?;

        match action {
            MenuAction::Open(index) => {
                println!();
                run_menu_action(index, projects, &doctor, unused_days)?;
                println!();
                println!("Press any key to return to the menu, or q to quit.");
                crossterm::terminal::enable_raw_mode()?;
                let key = read_key_code();
                let result = crossterm::terminal::disable_raw_mode();
                result?;
                let key = key?;
                if matches!(key, KeyCode::Char('q') | KeyCode::Esc) {
                    println!("Done.");
                    break;
                }
                first_render = true;
            }
            MenuAction::Refresh => {
                first_render = true;
            }
            MenuAction::Quit => {
                println!();
                println!("Done.");
                break;
            }
            MenuAction::Redraw => {}
        }
    }

    Ok(())
}

/// Render the keyboard dashboard frame, rewinding the cursor on
/// subsequent frames.
fn render_keyboard_menu<W: Write>(
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
    selected: usize,
    first_render: bool,
    output: &mut W,
) -> Result<()> {
    if !first_render {
        execute!(
            output,
            MoveUp(KEYBOARD_MENU_LINES),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
    }
    let width = terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(100)
        .max(40);

    let summary = summarize_fleet(projects);
    let largest = doctor
        .largest_project
        .as_ref()
        .map(|project| format!("{} {}", project.project, format_bytes(project.total)))
        .unwrap_or_else(|| "none".to_string());

    write_menu_line(output, "", width)?;
    queue!(
        output,
        SetForegroundColor(Color::Green),
        SetAttribute(Attribute::Bold),
        Print("LeanMgr"),
        ResetColor,
        SetAttribute(Attribute::Reset),
        Print("\n")
    )?;
    write_menu_line(output, "Manage disposable .lake caches", width)?;
    write_menu_line(output, "", width)?;
    write_metric_line(
        output,
        &[
            ("Projects", summary.project_count.to_string()),
            (
                "Cached",
                format!("{}/{}", summary.measured_count, summary.project_count),
            ),
            ("Reclaim", format_bytes(doctor.potential_hard_reclaim)),
            ("Unused", doctor.unused_projects.len().to_string()),
        ],
        width,
    )?;
    write_menu_line(
        output,
        &format!(
            "Missing paths {} | missing Lake files {} | largest {} | threshold {}d",
            doctor.missing_paths.len(),
            doctor.missing_lake_files.len(),
            largest,
            unused_days
        ),
        width,
    )?;
    write_menu_line(output, "", width)?;

    for (index, (label, description)) in KEYBOARD_ACTIONS.iter().enumerate() {
        let line = format!(
            "{}. {:<18} {}",
            index + 1,
            label,
            truncate_display(description, width.saturating_sub(26).max(12))
        );
        if index == selected {
            queue!(
                output,
                SetForegroundColor(Color::Green),
                SetAttribute(Attribute::Bold),
                Print("> "),
                Print(truncate_display(&line, width.saturating_sub(2))),
                ResetColor,
                SetAttribute(Attribute::Reset),
                Print("\n")
            )?;
        } else {
            write_menu_line(output, &format!("  {line}"), width)?;
        }
    }
    write_menu_line(output, "", width)?;
    write_menu_line(
        output,
        "Keys: Up/Down or j/k move | Enter open | 1-5 direct | r refresh | q quit",
        width,
    )?;
    output.flush()?;
    Ok(())
}

/// Block until the user picks an action, with arrow-key navigation
/// over `KEYBOARD_ACTIONS`.
fn read_menu_action(selected: &mut usize) -> Result<MenuAction> {
    loop {
        match read_key_code()? {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(MenuAction::Quit),
            KeyCode::Char('r') => return Ok(MenuAction::Refresh),
            KeyCode::Char(value @ '1'..='5') => {
                return Ok(MenuAction::Open(value as usize - '1' as usize));
            }
            KeyCode::Enter => return Ok(MenuAction::Open(*selected)),
            KeyCode::Up | KeyCode::Char('k') => {
                *selected = selected.saturating_sub(1);
                return Ok(MenuAction::Redraw);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if *selected + 1 < KEYBOARD_ACTIONS.len() {
                    *selected += 1;
                }
                return Ok(MenuAction::Redraw);
            }
            _ => {}
        }
    }
}

/// Read one keypress; ignore non-key events.
fn read_key_code() -> Result<KeyCode> {
    loop {
        if let Event::Key(key) = event::read()? {
            return Ok(key.code);
        }
    }
}

/// Dispatch one of the action renderers based on the menu index.
fn run_menu_action(
    index: usize,
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
) -> Result<()> {
    let input = std::io::stdin();
    let mut input = input.lock();
    let mut output = std::io::stdout();
    match index {
        0 => print_project_list(projects, &mut output),
        1 => print_doctor_summary(doctor, unused_days, &mut output),
        2 => print_gc_dry_run(projects, unused_days, &mut output),
        3 => print_clean_dry_run(projects, &mut input, &mut output),
        4 => print_restore_command(projects, &mut input, &mut output),
        _ => Ok(()),
    }
}
