//! Output helpers for human-readable and JSON output.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{self, Write};

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
/// Widths are measured in characters, matching how `{:width$}` pads. The
/// last column is not padded so lines carry no trailing whitespace.
pub fn write_table<W: Write>(
    output: &mut W,
    headers: &[&str],
    rows: &[Vec<String>],
) -> io::Result<()> {
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|header| header.chars().count())
        .collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
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
            write!(output, "{cell:width$}  ", width = widths[index])?;
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
        elapsed if elapsed < 365 * DAY => format!("{}mo ago", elapsed / (30 * DAY)),
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
    let split = trimmed
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let number = number.trim();
    // Pure integers (no `.`, `e`, `E`) parse strictly as u64 so overflow is
    // caught. Decimals fall back to f64, which is exact for small values and
    // checked against u64::MAX below.
    let value: f64 = if number.contains('.') || number.contains('e') || number.contains('E') {
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
    fn write_table_pads_by_characters_not_bytes() {
        // "数学" is 6 bytes but 2 characters; padding by byte length
        // would push the next column 4 columns too far right.
        let mut out = Vec::new();
        write_table(
            &mut out,
            &["NAME", "TAGS"],
            &[vec!["数学".to_string(), "x".to_string()]],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, "NAME  TAGS\n数学    x\n");
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
