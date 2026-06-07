//! Output helpers for human-readable and JSON output.

use anyhow::{Context, Result};
use serde::Serialize;

/// Print a serializable value as pretty JSON.
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Print a simple table.
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.len()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.len());
        }
    }

    for (index, header) in headers.iter().enumerate() {
        print!("{header:width$}", width = widths[index]);
        if index + 1 < headers.len() {
            print!("  ");
        }
    }
    println!();

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            print!("{cell:width$}", width = widths[index]);
            if index + 1 < row.len() {
                print!("  ");
            }
        }
        println!();
    }
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
    fn parse_bytes_rejects_overflow() {
        // 2^64 bytes — float parsing rounds up to a value that would saturate
        // to u64::MAX on the final cast. Must error, not silently saturate.
        assert!(parse_bytes("18446744073709551616").is_err());
        assert!(parse_bytes("1e20").is_err());
        assert!(parse_bytes("1e20B").is_err());
        assert!(parse_bytes("1e15TiB").is_err());
    }
}
