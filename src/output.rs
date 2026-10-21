//! Output helpers for human-readable and JSON output.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{self, Write};
use unicode_width::UnicodeWidthStr;

/// Print a serializable value as pretty JSON.
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Print a simple table to stdout.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    // Writing to stdout only fails when the reader went away (e.g. a closed
    // pipe); there is nobody left to report that to.
    let _ = write_table(&mut io::stdout().lock(), headers, rows);
}

/// Write a table with left-aligned, space-separated columns.
///
/// Widths are terminal display columns, so wide characters (CJK) line up.
/// The last column is not padded so lines carry no trailing whitespace.
pub fn write_table<W: Write>(
    output: &mut W,
    headers: &[&str],
    rows: &[Vec<String>],
) -> io::Result<()> {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.width()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.width());
        }
    }

    write_row(output, headers, &widths)?;
    for row in rows {
        let cells: Vec<&str> = row.iter().map(String::as_str).collect();
        write_row(output, &cells, &widths)?;
    }
    Ok(())
}

fn write_row<W: Write>(output: &mut W, cells: &[&str], widths: &[usize]) -> io::Result<()> {
    for (index, cell) in cells.iter().enumerate() {
        if index + 1 < cells.len() {
            let padding = widths[index].saturating_sub(cell.width());
            write!(output, "{cell}{:padding$}  ", "")?;
        } else {
            write!(output, "{cell}")?;
        }
    }
    writeln!(output)
}

/// Describe how long ago a stored epoch-seconds timestamp was, such as
/// `5m ago` or `3d ago`. Values that are not epoch seconds are returned
/// unchanged.
pub fn format_age(stored: &str, now: u64) -> String {
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let Ok(then) = stored.trim().parse::<u64>() else {
        return stored.to_string();
    };
    match now.saturating_sub(then) {
        elapsed if elapsed < MINUTE => "just now".to_string(),
        elapsed if elapsed < HOUR => format!("{}m ago", elapsed / MINUTE),
        elapsed if elapsed < DAY => format!("{}h ago", elapsed / HOUR),
        elapsed if elapsed < 30 * DAY => format!("{}d ago", elapsed / DAY),
        // 30-day months would reach "12mo" for days 360-364; cap below a year.
        elapsed if elapsed < 365 * DAY => format!("{}mo ago", (elapsed / (30 * DAY)).min(11)),
        elapsed => format!("{}y ago", elapsed / (365 * DAY)),
    }
}

/// Print a human-readable section heading.
pub fn print_section(title: &str) {
    println!();
    println!("{title}");
    println!("{}", "-".repeat(title.len()));
}

/// Format byte counts with binary units.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Parse a human byte size like `512`, `20GiB`, `2 GB` into a byte count.
///
/// Units are case-insensitive and treated as binary multipliers (`KB` == `KiB`).
pub fn parse_bytes(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("empty size: {input:?}");
    }
    let split = split_number_and_unit(trimmed);
    let (number, unit) = trimmed.split_at(split);
    let number = number.trim();
    // Pure integers (no `.`, `e`, `E`, sign) parse strictly as u64 so
    // overflow is caught. Everything else falls back to f64, which is exact
    // for small values and checked against u64::MAX below.
    let value: f64 = if number.contains('.')
        || number.contains('e')
        || number.contains('E')
        || number.starts_with('-')
        || number.starts_with('+')
    {
        number
            .parse()
            .with_context(|| format!("invalid size number in {input:?}"))?
    } else {
        let int: u64 = number
            .parse()
            .with_context(|| format!("invalid size number in {input:?}"))?;
        int as f64
    };
    if value < 0.0 {
        anyhow::bail!("size must be non-negative: {input:?}");
    }
    let multiplier: f64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0_f64.powi(3),
        "t" | "tb" | "tib" => 1024.0_f64.powi(4),
        other => anyhow::bail!("unknown size unit {other:?} in {input:?}"),
    };
    let product = value * multiplier;
    if !product.is_finite() {
        anyhow::bail!("size is not finite: {input:?}");
    }
    if product > u64::MAX as f64 {
        anyhow::bail!("size exceeds u64::MAX: {input:?}");
    }
    Ok(product as u64)
}

/// Return the byte offset where the unit part of a size string starts.
///
/// The number part may contain digits, a decimal point, a sign, and a
/// scientific-notation exponent (`1.5e3`). The exponent marker is an `e`,
/// so the split cannot simply be "first alphabetic character" — that would
/// cut `1e20` into `1` and `e20` and reject the input as an unknown unit.
fn split_number_and_unit(input: &str) -> usize {
    let bytes = input.as_bytes();
    let mut index = 0;

    // Optional leading sign.
    if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        index += 1;
    }

    // Digits and a decimal point.
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
        index += 1;
    }

    // Optional exponent: `e` or `E`, optional sign, at least one digit.
    if index < bytes.len() && (bytes[index] == b'e' || bytes[index] == b'E') {
        let mut cursor = index + 1;
        if cursor < bytes.len() && (bytes[cursor] == b'+' || bytes[cursor] == b'-') {
            cursor += 1;
        }
        let digits_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor > digits_start {
            index = cursor;
        }
    }

    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytes_handles_units() {
        assert_eq!(parse_bytes("1024").unwrap(), 1024);
        assert_eq!(parse_bytes("1KiB").unwrap(), 1024);
        assert_eq!(parse_bytes("1 mib").unwrap(), 1024 * 1024);
        assert_eq!(parse_bytes("2GB").unwrap(), 2 * 1024 * 1024 * 1024);
        assert!(parse_bytes("abc").is_err());
        assert!(parse_bytes("").is_err());
    }

    /// Scientific notation belongs to the number, not the unit. Splitting on
    /// the first alphabetic character used to cut `1e20` into `1` + `e20`
    /// and reject it as an unknown unit.
    #[test]
    fn parse_bytes_accepts_scientific_notation() {
        assert_eq!(parse_bytes("1e3").unwrap(), 1000);
        assert_eq!(parse_bytes("1.5e3").unwrap(), 1500);
        assert_eq!(parse_bytes("2e2KiB").unwrap(), 200 * 1024);
        assert_eq!(parse_bytes("1E3").unwrap(), 1000);
    }

    #[test]
    fn parse_bytes_rejects_negative_and_malformed() {
        assert!(parse_bytes("-5").is_err());
        assert!(parse_bytes("-1GiB").is_err());
        assert!(parse_bytes("1e").is_err());
        assert!(parse_bytes("1ex").is_err());
        assert!(parse_bytes("KB").is_err());
    }

    #[test]
    fn write_table_aligns_columns_without_trailing_space() {
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["NAME", "SIZE"],
            &[
                vec!["a".to_string(), "1 B".to_string()],
                vec!["longer".to_string(), "2.0 KiB".to_string()],
            ],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "NAME    SIZE\na       1 B\nlonger  2.0 KiB\n");
    }

    #[test]
    fn write_table_pads_by_display_width() {
        // "数学" is 6 bytes and 2 characters but 4 terminal columns wide;
        // only display width keeps the next column aligned.
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["NAME", "TAGS"],
            &[
                vec!["数学".to_string(), "x".to_string()],
                vec!["é".to_string(), "y".to_string()],
            ],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "NAME  TAGS\n数学  x\né     y\n");
    }

    #[test]
    fn format_age_buckets_elapsed_time() {
        let now = 1_000_000_000;
        assert_eq!(format_age(&now.to_string(), now), "just now");
        assert_eq!(format_age(&(now - 59).to_string(), now), "just now");
        assert_eq!(format_age(&(now - 5 * 60).to_string(), now), "5m ago");
        assert_eq!(format_age(&(now - 3 * 3600).to_string(), now), "3h ago");
        assert_eq!(format_age(&(now - 2 * 86_400).to_string(), now), "2d ago");
        assert_eq!(format_age(&(now - 90 * 86_400).to_string(), now), "3mo ago");
        assert_eq!(
            format_age(&(now - 362 * 86_400).to_string(), now),
            "11mo ago"
        );
        assert_eq!(format_age(&(now - 365 * 86_400).to_string(), now), "1y ago");
        assert_eq!(format_age(&(now - 800 * 86_400).to_string(), now), "2y ago");
    }

    #[test]
    fn format_age_tolerates_clock_skew_and_foreign_values() {
        assert_eq!(format_age("2000", 1000), "just now");
        assert_eq!(
            format_age("2026-06-07T00:00:00Z", 0),
            "2026-06-07T00:00:00Z"
        );
    }

    #[test]
    fn parse_bytes_rejects_overflow() {
        // 2^64 bytes — float parsing rounds up to a value that would saturate
        // to u64::MAX on the final cast. Must error, not silently saturate.
        assert!(parse_bytes("18446744073709551616").is_err());
        assert!(parse_bytes("1e20").is_err());
        assert!(parse_bytes("1e20B").is_err());
        assert!(parse_bytes("1e15TiB").is_err());
    }
}
