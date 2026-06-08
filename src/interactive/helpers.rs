//! Low-level output and parsing helpers shared by the dashboard, action
//! renderers, and keyboard session. These are deliberately parameterised
//! over any `W: Write` so the pipe-mode session can write to stdout and
//! the keyboard session can write to its terminal writer.

use crate::cli::CleanLevel;
use anyhow::{Result, bail};
use std::io::{BufRead, Write};

/// Read one line of input from the user, prefixed with `label> `.
pub(super) fn prompt<R: BufRead, W: Write>(
    label: &str,
    input: &mut R,
    output: &mut W,
) -> Result<String> {
    write!(output, "{label}> ")?;
    output.flush()?;
    let mut value = String::new();
    input.read_line(&mut value)?;
    Ok(value)
}

/// Print a section heading with a rule underneath. Used at the top of
/// every action renderer's output.
pub(super) fn print_section_to<W: Write>(output: &mut W, title: &str) -> Result<()> {
    writeln!(output)?;
    writeln!(output, "{title}")?;
    writeln!(output, "{}", "-".repeat(title.len()))?;
    Ok(())
}

/// Print a list of names under a title; no-op when the list is empty.
pub(super) fn write_name_list<W: Write>(
    output: &mut W,
    title: &str,
    names: &[String],
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }
    writeln!(output, "{title}:")?;
    for name in names {
        writeln!(output, "  {name}")?;
    }
    Ok(())
}

/// Render a 2-column table of `headers` + `rows`, padding each cell so
/// columns line up. Mirrors `crate::output::print_table` but takes a
/// `Write` so it works inside the interactive session writers.
pub(super) fn print_table_to<W: Write>(
    output: &mut W,
    headers: &[&str],
    rows: &[Vec<String>],
) -> Result<()> {
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

/// Truncate `line` to `width` columns, appending `...` when it was longer.
pub(super) fn truncate_display(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let prefix = line.chars().take(width - 3).collect::<String>();
    format!("{prefix}...")
}

/// Write a single menu line, truncating to `width` columns.
pub(super) fn write_menu_line<W: Write>(output: &mut W, line: &str, width: usize) -> Result<()> {
    writeln!(output, "{}", truncate_display(line, width))?;
    Ok(())
}

/// Write a single metrics line of the form
/// `[label1 value1]  [label2 value2]  ...`, truncating to `width` columns.
pub(super) fn write_metric_line<W: Write>(
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

/// Map a `CleanLevel` to its CLI argument spelling.
pub(super) fn clean_level_arg(level: CleanLevel) -> &'static str {
    match level {
        CleanLevel::Soft => "soft",
        CleanLevel::DepsBuild => "deps-build",
        CleanLevel::Hard => "hard",
    }
}

/// Quote `value` for use in a shell command, single-quoting when the
/// value contains characters the shell would interpret.
pub(super) fn shell_arg(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Parse a clean level from a free-form prompt answer.
pub(super) fn parse_clean_level(input: &str) -> Result<CleanLevel> {
    match input {
        "soft" => Ok(CleanLevel::Soft),
        "deps-build" | "deps_build" => Ok(CleanLevel::DepsBuild),
        "hard" => Ok(CleanLevel::Hard),
        other => bail!("unknown clean level: {other}"),
    }
}
