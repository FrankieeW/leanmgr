//! Output helpers for human-readable and JSON output.

use anyhow::Result;
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
