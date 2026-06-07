//! External process helpers.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Output};

/// Run a command in a directory and capture its output.
pub fn run_in(dir: &Path, program: &str, args: &[&str]) -> Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("failed to run {program} in {}", dir.display()))
}

/// Run a command in the current directory and capture stdout as text.
pub fn stdout(program: &str, args: &[&str]) -> Result<Option<String>> {
    let output = match Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };
    if !output.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
}
