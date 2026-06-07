//! Multi-project toolchain audit commands.

use crate::cli::ToolchainCommands;
use crate::config::load_config;
use crate::output::print_table;
use crate::process::stdout;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

/// Run a toolchain command.
pub fn toolchain_command(command: ToolchainCommands) -> Result<()> {
    match command {
        ToolchainCommands::List => list_toolchains(),
        ToolchainCommands::Check => check_toolchains(),
    }
}

fn list_toolchains() -> Result<()> {
    let counts = referenced_toolchains()?;
    let rows: Vec<Vec<String>> = counts
        .into_iter()
        .map(|(toolchain, count)| vec![toolchain, count.to_string()])
        .collect();
    print_table(&["TOOLCHAIN", "PROJECTS"], &rows);
    Ok(())
}

fn check_toolchains() -> Result<()> {
    let counts = referenced_toolchains()?;
    let installed = installed_toolchains()?;
    let rows: Vec<Vec<String>> = counts
        .into_iter()
        .map(|(toolchain, count)| {
            let status = if installed.contains(&toolchain) {
                "installed"
            } else {
                "missing"
            };
            vec![toolchain, count.to_string(), status.to_string()]
        })
        .collect();
    print_table(&["TOOLCHAIN", "PROJECTS", "STATUS"], &rows);
    Ok(())
}

fn referenced_toolchains() -> Result<BTreeMap<String, usize>> {
    let config = load_config()?;
    let mut counts = BTreeMap::new();
    for project in config.projects {
        let path = project.expanded_path().join("lean-toolchain");
        let toolchain = fs::read_to_string(path)
            .ok()
            .and_then(|content| content.lines().next().map(str::trim).map(str::to_string))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "missing".to_string());
        *counts.entry(toolchain).or_insert(0) += 1;
    }
    Ok(counts)
}

fn installed_toolchains() -> Result<BTreeSet<String>> {
    let Some(output) = stdout("elan", &["toolchain", "list"])? else {
        return Ok(BTreeSet::new());
    };
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.trim_end_matches(" (default)").to_string())
        .collect())
}
