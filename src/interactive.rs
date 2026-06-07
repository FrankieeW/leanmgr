//! Interactive assistant for cache-management decisions.

use crate::clean::build_clean_plan;
use crate::cli::{CleanLevel, InteractArgs};
use crate::config::load_config;
use crate::doctor::{DoctorReport, build_doctor_report};
use crate::gc::{GcMode, GcOptions, plan_gc};
use crate::output::format_bytes;
use crate::project::{Project, filter_by_tag};
use anyhow::{Context, Result, bail};
use crossterm::cursor::{MoveToColumn, MoveUp};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, Clear, ClearType, disable_raw_mode, enable_raw_mode};
use std::collections::BTreeMap;
use std::io::{self, BufRead, IsTerminal, Write};

/// High-level fleet summary for interactive display.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetSummary {
    /// Number of projects in the current assistant scope.
    pub project_count: usize,
    /// Number of projects with cached size values.
    pub measured_count: usize,
    /// Sum of cached `.lake` sizes.
    pub cached_total: u64,
    /// Tag usage counts.
    pub tags: BTreeMap<String, usize>,
}

/// Start the interactive assistant.
pub fn interact_command(args: InteractArgs) -> Result<()> {
    let config = load_config()?;
    let projects: Vec<Project> = filter_by_tag(&config.projects, args.tag.as_deref())
        .into_iter()
        .cloned()
        .collect();
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        return run_keyboard_session(&projects, args.unused_days);
    }
    let input = io::stdin();
    let mut output = io::stdout();
    run_session(&projects, args.unused_days, input.lock(), &mut output)
}

/// Run an interactive assistant session over a prepared project scope.
pub fn run_session<R: BufRead, W: Write>(
    projects: &[Project],
    unused_days: u64,
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

    loop {
        let doctor = build_doctor_report(projects, unused_days)?;
        print_dashboard(projects, &doctor, unused_days, output)?;
        print_menu(output)?;
        let choice = prompt("Choose an action", &mut input, output)?;

        match choice.trim() {
            "1" => print_project_list(projects, output)?,
            "2" => print_doctor_summary(&doctor, unused_days, output)?,
            "3" => print_gc_dry_run(projects, unused_days, output)?,
            "4" => print_clean_dry_run(projects, &mut input, output)?,
            "5" => print_restore_command(projects, &mut input, output)?,
            "6" => continue,
            "q" | "Q" | "quit" | "exit" => {
                writeln!(output, "Done.")?;
                break;
            }
            other => writeln!(output, "Unknown action: {other}")?,
        }

        let _ = prompt("Press Enter to continue", &mut input, output)?;
    }

    Ok(())
}

/// Build a cache summary from indexed project metadata.
pub fn summarize_fleet(projects: &[Project]) -> FleetSummary {
    let mut tags = BTreeMap::new();
    let mut measured_count = 0usize;
    let mut cached_total = 0u64;

    for project in projects {
        if let Some(cache) = &project.size_cache {
            measured_count += 1;
            cached_total += cache.total;
        }
        for tag in &project.tags {
            *tags.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    FleetSummary {
        project_count: projects.len(),
        measured_count,
        cached_total,
        tags,
    }
}

fn run_keyboard_session(projects: &[Project], unused_days: u64) -> Result<()> {
    if projects.is_empty() {
        println!("No indexed projects in this scope.");
        println!("Add projects with `leanmgr add <path>` or `leanmgr scan <root>`.");
        return Ok(());
    }

    let mut output = io::stdout();
    let mut selected = 0usize;
    let mut first_render = true;

    loop {
        let doctor = build_doctor_report(projects, unused_days)?;
        render_keyboard_menu(
            projects,
            &doctor,
            unused_days,
            selected,
            first_render,
            &mut output,
        )?;
        first_render = false;

        enable_raw_mode()?;
        let action = read_menu_action(&mut selected)?;
        disable_raw_mode()?;

        match action {
            MenuAction::Open(index) => {
                println!();
                run_menu_action(index, projects, &doctor, unused_days)?;
                println!();
                println!("Press any key to return to the menu, or q to quit.");
                enable_raw_mode()?;
                let quit = matches!(read_key_code()?, KeyCode::Char('q') | KeyCode::Esc);
                disable_raw_mode()?;
                if quit {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MenuAction {
    Open(usize),
    Refresh,
    Quit,
    Redraw,
}

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
const KEYBOARD_MENU_LINES: u16 = 13;

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

fn write_metric_line<W: Write>(
    output: &mut W,
    metrics: &[(&str, String)],
    width: usize,
) -> Result<()> {
    let mut line = String::new();
    for (index, (label, value)) in metrics.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        line.push('[');
        line.push_str(label);
        line.push(' ');
        line.push_str(value);
        line.push(']');
    }
    write_menu_line(output, &line, width)
}

fn write_menu_line<W: Write>(output: &mut W, line: &str, width: usize) -> Result<()> {
    writeln!(output, "{}", truncate_display(line, width))?;
    Ok(())
}

fn truncate_display(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let prefix = line.chars().take(width - 3).collect::<String>();
    format!("{prefix}...")
}

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

fn read_key_code() -> Result<KeyCode> {
    loop {
        if let Event::Key(key) = event::read()? {
            return Ok(key.code);
        }
    }
}

fn run_menu_action(
    index: usize,
    projects: &[Project],
    doctor: &DoctorReport,
    unused_days: u64,
) -> Result<()> {
    let input = io::stdin();
    let mut input = input.lock();
    let mut output = io::stdout();
    match index {
        0 => print_project_list(projects, &mut output),
        1 => print_doctor_summary(doctor, unused_days, &mut output),
        2 => print_gc_dry_run(projects, unused_days, &mut output),
        3 => print_clean_dry_run(projects, &mut input, &mut output),
        4 => print_restore_command(projects, &mut input, &mut output),
        _ => Ok(()),
    }
}

fn print_dashboard<W: Write>(
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

fn print_menu<W: Write>(output: &mut W) -> Result<()> {
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

fn print_project_list<W: Write>(projects: &[Project], output: &mut W) -> Result<()> {
    print_section_to(output, "Projects")?;
    let rows: Vec<Vec<String>> = projects
        .iter()
        .enumerate()
        .map(|(index, project)| {
            let size = project
                .size_cache
                .as_ref()
                .map(|cache| format_bytes(cache.total))
                .unwrap_or_else(|| "unknown".to_string());
            vec![
                (index + 1).to_string(),
                project.name.clone(),
                project.tags.join(","),
                size,
                project.path.clone(),
            ]
        })
        .collect();
    print_table_to(output, &["#", "NAME", "TAGS", "CACHED SIZE", "PATH"], &rows)?;
    Ok(())
}

fn print_doctor_summary<W: Write>(
    doctor: &DoctorReport,
    unused_days: u64,
    output: &mut W,
) -> Result<()> {
    print_section_to(output, "Doctor details")?;
    writeln!(
        output,
        "Potential reclaim: {}",
        format_bytes(doctor.potential_hard_reclaim)
    )?;
    if let Some(project) = &doctor.largest_project {
        writeln!(
            output,
            "Largest project: {} {}",
            project.project,
            format_bytes(project.total)
        )?;
    }
    if !doctor.unused_projects.is_empty() {
        writeln!(output, "Unused > {unused_days} days:")?;
        for project in &doctor.unused_projects {
            writeln!(
                output,
                "  {} {}",
                project.project,
                format_bytes(project.total)
            )?;
        }
    }
    write_name_list(output, "Missing paths", &doctor.missing_paths)?;
    write_name_list(output, "Missing Lake files", &doctor.missing_lake_files)?;
    if !doctor.toolchains.is_empty() {
        writeln!(output, "Toolchains:")?;
        for (toolchain, count) in &doctor.toolchains {
            writeln!(output, "  {toolchain} {count}")?;
        }
    }
    Ok(())
}

fn print_gc_dry_run<W: Write>(
    projects: &[Project],
    unused_days: u64,
    output: &mut W,
) -> Result<()> {
    let opts = GcOptions {
        mode: GcMode::UnusedDays(unused_days),
        level: CleanLevel::Hard,
        include_unrecoverable: false,
    };
    let (targets, skipped) = plan_gc(projects, &opts)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    print_section_to(output, "GC dry-run")?;
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|target| {
            vec![
                target.project.clone(),
                format_bytes(target.bytes),
                target.path.display().to_string(),
            ]
        })
        .collect();
    print_table_to(output, &["PROJECT", "SIZE", "WOULD REMOVE"], &rows)?;
    writeln!(output, "Total reclaimable: {}", format_bytes(total))?;
    if !skipped.is_empty() {
        writeln!(output, "Skipped as unrecoverable:")?;
        for skip in skipped {
            writeln!(output, "  {} {}", skip.project, skip.reason)?;
        }
    }
    writeln!(
        output,
        "To execute after review: leanmgr gc --unused-days {unused_days} --level hard"
    )?;
    Ok(())
}

fn print_clean_dry_run<R: BufRead, W: Write>(
    projects: &[Project],
    input: &mut R,
    output: &mut W,
) -> Result<()> {
    let project = choose_project(projects, input, output)?;
    let level = choose_clean_level(input, output)?;
    let targets = build_clean_plan(std::slice::from_ref(project), level)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    print_section_to(output, "Clean dry-run")?;
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|target| {
            vec![
                target.project.clone(),
                format_bytes(target.bytes),
                target.path.display().to_string(),
            ]
        })
        .collect();
    print_table_to(output, &["PROJECT", "SIZE", "WOULD REMOVE"], &rows)?;
    writeln!(output, "Total reclaimable: {}", format_bytes(total))?;
    writeln!(
        output,
        "To execute after review: leanmgr clean {} --level {}",
        shell_arg(&project.name),
        clean_level_arg(level)
    )?;
    Ok(())
}

fn print_restore_command<R: BufRead, W: Write>(
    projects: &[Project],
    input: &mut R,
    output: &mut W,
) -> Result<()> {
    let project = choose_project(projects, input, output)?;
    print_section_to(output, "Restore command")?;
    writeln!(
        output,
        "Run this when you want Lake to restore cache artifacts:"
    )?;
    writeln!(output, "leanmgr restore {}", shell_arg(&project.name))?;
    Ok(())
}

fn choose_project<'a, R: BufRead, W: Write>(
    projects: &'a [Project],
    input: &mut R,
    output: &mut W,
) -> Result<&'a Project> {
    print_project_list(projects, output)?;
    let selector = prompt("Project number or name", input, output)?;
    let trimmed = selector.trim();
    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 {
            bail!("project index out of range: {index}");
        }
        return projects
            .get(index - 1)
            .with_context(|| format!("project index out of range: {index}"));
    }
    projects
        .iter()
        .find(|project| project.name == trimmed)
        .with_context(|| format!("project not found: {trimmed}"))
}

fn choose_clean_level<R: BufRead, W: Write>(input: &mut R, output: &mut W) -> Result<CleanLevel> {
    writeln!(output, "Clean levels: soft, deps-build, hard")?;
    let level = prompt("Clean level", input, output)?;
    parse_clean_level(level.trim())
}

fn parse_clean_level(input: &str) -> Result<CleanLevel> {
    match input {
        "soft" => Ok(CleanLevel::Soft),
        "deps-build" | "deps_build" => Ok(CleanLevel::DepsBuild),
        "hard" => Ok(CleanLevel::Hard),
        other => bail!("unknown clean level: {other}"),
    }
}

fn prompt<R: BufRead, W: Write>(label: &str, input: &mut R, output: &mut W) -> Result<String> {
    write!(output, "{label}> ")?;
    output.flush()?;
    let mut value = String::new();
    input.read_line(&mut value)?;
    Ok(value)
}

fn write_name_list<W: Write>(output: &mut W, title: &str, names: &[String]) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    writeln!(output, "{title}:")?;
    for name in names {
        writeln!(output, "  {name}")?;
    }
    Ok(())
}

fn print_section_to<W: Write>(output: &mut W, title: &str) -> Result<()> {
    writeln!(output)?;
    writeln!(output, "{title}")?;
    writeln!(output, "{}", "-".repeat(title.len()))?;
    Ok(())
}

fn print_table_to<W: Write>(output: &mut W, headers: &[&str], rows: &[Vec<String>]) -> Result<()> {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.len());
        }
    }

    for (index, header) in headers.iter().enumerate() {
        write!(output, "{header:width$}", width = widths[index])?;
        if index + 1 < headers.len() {
            write!(output, "  ")?;
        }
    }
    writeln!(output)?;

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            write!(output, "{cell:width$}", width = widths[index])?;
            if index + 1 < row.len() {
                write!(output, "  ")?;
            }
        }
        writeln!(output)?;
    }
    Ok(())
}

fn clean_level_arg(level: CleanLevel) -> &'static str {
    match level {
        CleanLevel::Soft => "soft",
        CleanLevel::DepsBuild => "deps-build",
        CleanLevel::Hard => "hard",
    }
}

fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::SizeCache;

    #[test]
    fn summarize_fleet_counts_cached_sizes_and_tags() {
        let projects = vec![
            project("one", vec!["active"], Some(1024)),
            project("two", vec!["active", "archive"], None),
        ];
        let summary = summarize_fleet(&projects);

        assert_eq!(summary.project_count, 2);
        assert_eq!(summary.measured_count, 1);
        assert_eq!(summary.cached_total, 1024);
        assert_eq!(summary.tags.get("active"), Some(&2));
        assert_eq!(summary.tags.get("archive"), Some(&1));
    }

    #[test]
    fn parse_clean_level_accepts_cli_names() {
        assert_eq!(parse_clean_level("soft").unwrap(), CleanLevel::Soft);
        assert_eq!(
            parse_clean_level("deps-build").unwrap(),
            CleanLevel::DepsBuild
        );
        assert_eq!(
            parse_clean_level("deps_build").unwrap(),
            CleanLevel::DepsBuild
        );
        assert_eq!(parse_clean_level("hard").unwrap(), CleanLevel::Hard);
        assert!(parse_clean_level("deep").is_err());
    }

    #[test]
    fn shell_arg_quotes_spaces_and_single_quotes() {
        assert_eq!(shell_arg("demo"), "demo");
        assert_eq!(shell_arg("two words"), "'two words'");
        assert_eq!(shell_arg("bob's"), "'bob'\\''s'");
    }

    fn project(name: &str, tags: Vec<&str>, total: Option<u64>) -> Project {
        Project {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            tags: tags.into_iter().map(str::to_string).collect(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: total.map(|total| SizeCache {
                lake: total,
                build: 0,
                packages: 0,
                total,
                computed_at: "1".to_string(),
            }),
        }
    }
}
