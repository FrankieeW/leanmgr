//! Keyboard-mode interactive session. Renders a highlighted menu, reads
//! a single keypress at a time, and dispatches to the action renderers
//! in `actions.rs`.

use super::actions::{
    print_clean_dry_run, print_doctor_summary, print_gc_dry_run, print_project_list,
    print_restore_command,
};
use super::helpers::{truncate_display, write_menu_line, write_metric_line};
use super::summary::summarize_fleet;
use crate::doctor::{DoctorReport, build_doctor_report};
use crate::output::format_bytes;
use crate::project::Project;
use anyhow::Result;
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
/// indices used by `menu_action_for` and `run_menu_action`.
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

/// Run the keyboard dashboard for the duration of one session.
pub(super) fn run_keyboard_session(
    projects: &[Project],
    unused_days: u64,
    tag: Option<&str>,
) -> Result<()> {
    if projects.is_empty() {
        println!("No indexed projects in this scope.");
        println!("Add projects with `leanmgr add <path>` or `leanmgr scan <root>`.");
        return Ok(());
    }

    let mut output = std::io::stdout();
    let mut selected = 0usize;
    // Lines drawn by the previous frame; a redraw rewinds exactly this many.
    let mut drawn_lines = 0u16;
    // Walking every `.lake` is the slow part, so the report is built once
    // and rebuilt only on an explicit refresh, never on navigation keys.
    let mut doctor = build_doctor_report(projects, unused_days)?;

    loop {
        drawn_lines = render_keyboard_menu(
            projects,
            &doctor,
            unused_days,
            selected,
            drawn_lines,
            &mut output,
        )?;

        match with_raw_mode(|| read_menu_action(&mut selected))? {
            MenuAction::Open(index) => {
                println!();
                // A bad answer inside an action must not end the session.
                if let Err(error) = run_menu_action(index, projects, &doctor, unused_days, tag) {
                    println!("Error: {error:#}");
                }
                println!();
                println!("Press any key to return to the menu, or q to quit.");
                if is_quit_key(with_raw_mode(read_key)?) {
                    println!("Done.");
                    break;
                }
                drawn_lines = 0;
            }
            MenuAction::Refresh => {
                doctor = build_doctor_report(projects, unused_days)?;
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

/// Run `read` with the terminal in raw mode, restoring cooked mode even
/// when `read` fails.
fn with_raw_mode<T>(read: impl FnOnce() -> Result<T>) -> Result<T> {
    terminal::enable_raw_mode()?;
    let result = read();
    terminal::disable_raw_mode()?;
    result
}

/// Draw the dashboard frame and return how many lines it used. When
/// `previous_lines` is non-zero the previous frame is cleared first, so
/// navigation redraws in place without a fullscreen alternate screen and
/// output stays in normal scrollback.
fn render_keyboard_menu<W: Write>(
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
    selected: usize,
    previous_lines: u16,
    output: &mut W,
) -> Result<u16> {
    let width = frame_width(terminal::size().ok().map(|(columns, _)| columns));
    let mut frame = Vec::new();
    write_frame_header(&mut frame, projects, doctor, unused_days, width)?;
    write_frame_menu(&mut frame, selected, width)?;

    if previous_lines > 0 {
        execute!(
            output,
            MoveUp(previous_lines),
            MoveToColumn(0),
            Clear(ClearType::FromCursorDown)
        )?;
    }
    output.write_all(&frame)?;
    output.flush()?;
    Ok(line_count(&frame))
}

/// Columns each frame line may use. One column is left free because some
/// terminals wrap as soon as the last column is written, and the width is
/// never raised above the real terminal: a wrapped line would make the
/// redraw rewind one row too few.
fn frame_width(terminal_columns: Option<u16>) -> usize {
    const FALLBACK_COLUMNS: usize = 100;
    terminal_columns
        .map(usize::from)
        .filter(|columns| *columns > 0)
        .unwrap_or(FALLBACK_COLUMNS)
        .saturating_sub(1)
        .max(1)
}

fn line_count(frame: &[u8]) -> u16 {
    let lines = frame.iter().filter(|byte| **byte == b'\n').count();
    u16::try_from(lines).unwrap_or(u16::MAX)
}

/// Title and fleet metrics at the top of the frame.
fn write_frame_header<W: Write>(
    output: &mut W,
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
    width: usize,
) -> Result<()> {
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
    Ok(())
}

/// Action list with the selected row highlighted, plus the key legend.
fn write_frame_menu<W: Write>(output: &mut W, selected: usize, width: usize) -> Result<()> {
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
    Ok(())
}

/// Block until the user picks an action, with arrow-key navigation
/// over `KEYBOARD_ACTIONS`.
fn read_menu_action(selected: &mut usize) -> Result<MenuAction> {
    loop {
        if let Some(action) = menu_action_for(read_key()?, selected) {
            return Ok(action);
        }
    }
}

/// Map one keypress to a menu action, moving `selected` for navigation
/// keys. Returns `None` for keys the menu ignores.
fn menu_action_for(key: KeyEvent, selected: &mut usize) -> Option<MenuAction> {
    if is_quit_key(key) {
        return Some(MenuAction::Quit);
    }
    match key.code {
        KeyCode::Char('r') => Some(MenuAction::Refresh),
        KeyCode::Char(value @ '1'..='5') => Some(MenuAction::Open(value as usize - '1' as usize)),
        KeyCode::Enter => Some(MenuAction::Open(*selected)),
        KeyCode::Up | KeyCode::Char('k') => {
            *selected = selected.saturating_sub(1);
            Some(MenuAction::Redraw)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if *selected + 1 < KEYBOARD_ACTIONS.len() {
                *selected += 1;
            }
            Some(MenuAction::Redraw)
        }
        _ => None,
    }
}

/// `q`, Esc, and Ctrl-C quit. Raw mode delivers Ctrl-C as a key event
/// instead of SIGINT, so it has to be handled here.
fn is_quit_key(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// Block until a key is pressed. Key-release events (reported on Windows)
/// and non-key events are ignored so one physical press is handled once.
fn read_key() -> Result<KeyEvent> {
    loop {
        if let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            return Ok(key);
        }
    }
}

/// Dispatch one of the action renderers based on the menu index.
fn run_menu_action(
    index: usize,
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
    tag: Option<&str>,
) -> Result<()> {
    let input = std::io::stdin();
    let mut input = input.lock();
    let mut output = std::io::stdout();
    match index {
        0 => print_project_list(projects, &mut output),
        1 => print_doctor_summary(doctor, unused_days, &mut output),
        2 => print_gc_dry_run(projects, unused_days, tag, &mut output),
        3 => print_clean_dry_run(projects, &mut input, &mut output),
        4 => print_restore_command(projects, &mut input, &mut output),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn frame_line_count_matches_what_is_drawn() {
        // The redraw rewinds by the counted lines; a hard-coded count that
        // drifted from the real frame made the menu creep down one line per
        // keypress.
        let mut frame = Vec::new();
        write_frame_header(&mut frame, &[], &DoctorReport::default(), 90, 100).unwrap();
        write_frame_menu(&mut frame, 0, 100).unwrap();
        let text = String::from_utf8_lossy(&frame).to_string();
        assert_eq!(usize::from(line_count(&frame)), text.lines().count());
        assert_eq!(line_count(&frame), 14);
    }

    /// Remove ANSI escape sequences (`ESC [ ... letter`) so tests can
    /// measure what the terminal actually shows.
    fn visible(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn frame_lines_never_exceed_the_width_even_with_wide_characters() {
        use crate::size::ProjectSize;
        use unicode_width::UnicodeWidthStr;
        let doctor = DoctorReport {
            largest_project: Some(ProjectSize {
                project: "代数数论项目".repeat(8),
                total: 1 << 30,
                ..ProjectSize::default()
            }),
            ..DoctorReport::default()
        };
        for columns in [10u16, 24, 40, 80, 200] {
            let width = frame_width(Some(columns));
            let mut frame = Vec::new();
            write_frame_header(&mut frame, &[], &doctor, 90, width).unwrap();
            write_frame_menu(&mut frame, 1, width).unwrap();
            let text = visible(&String::from_utf8_lossy(&frame));
            for line in text.lines() {
                assert!(
                    line.width() < usize::from(columns),
                    "{columns} columns: {line:?} is {} wide",
                    line.width()
                );
            }
            assert_eq!(usize::from(line_count(&frame)), text.lines().count());
        }
    }

    #[test]
    fn frame_width_uses_the_real_terminal_and_a_fallback() {
        assert_eq!(frame_width(Some(80)), 79);
        assert_eq!(frame_width(Some(1)), 1);
        assert_eq!(frame_width(Some(0)), 99);
        assert_eq!(frame_width(None), 99);
    }

    #[test]
    fn frame_highlights_exactly_the_selected_action() {
        let mut frame = Vec::new();
        write_frame_menu(&mut frame, 2, 100).unwrap();
        let text = String::from_utf8_lossy(&frame).to_string();
        let marked: Vec<&str> = text.lines().filter(|line| line.contains("> ")).collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].contains("3. GC dry-run"));
    }

    #[test]
    fn navigation_keys_move_within_bounds() {
        let mut selected = 0;
        assert_eq!(
            menu_action_for(press(KeyCode::Up), &mut selected),
            Some(MenuAction::Redraw)
        );
        assert_eq!(selected, 0);
        for _ in 0..10 {
            menu_action_for(press(KeyCode::Char('j')), &mut selected);
        }
        assert_eq!(selected, KEYBOARD_ACTIONS.len() - 1);
        assert_eq!(
            menu_action_for(press(KeyCode::Enter), &mut selected),
            Some(MenuAction::Open(KEYBOARD_ACTIONS.len() - 1))
        );
    }

    #[test]
    fn digits_refresh_and_unknown_keys() {
        let mut selected = 0;
        assert_eq!(
            menu_action_for(press(KeyCode::Char('3')), &mut selected),
            Some(MenuAction::Open(2))
        );
        assert_eq!(
            menu_action_for(press(KeyCode::Char('r')), &mut selected),
            Some(MenuAction::Refresh)
        );
        assert_eq!(
            menu_action_for(press(KeyCode::Char('6')), &mut selected),
            None
        );
        assert_eq!(
            menu_action_for(press(KeyCode::Char('x')), &mut selected),
            None
        );
    }

    #[test]
    fn ctrl_c_quits_but_plain_c_does_not() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_quit_key(ctrl_c));
        assert!(is_quit_key(press(KeyCode::Char('q'))));
        assert!(is_quit_key(press(KeyCode::Esc)));
        assert!(!is_quit_key(press(KeyCode::Char('c'))));
    }
}
