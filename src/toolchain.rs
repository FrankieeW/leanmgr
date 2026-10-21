//! Multi-project toolchain audit commands.

use crate::cli::ToolchainCommands;
use crate::config::load_config;
use crate::lake::read_toolchain;
use crate::output::print_table;
use crate::process::stdout;
use crate::project::Project;
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

/// Row label for projects that have no `lean-toolchain` file.
const NO_TOOLCHAIN: &str = "(no lean-toolchain)";

/// Run a toolchain command.
pub fn toolchain_command(command: ToolchainCommands) -> Result<()> {
    let config = load_config()?;
    let census = ToolchainCensus::of(&config.projects);
    match command {
        ToolchainCommands::List => list_toolchains(&census),
        ToolchainCommands::Check => check_toolchains(&census, installed_toolchains().as_ref()),
    }
    Ok(())
}

/// Which toolchains the indexed projects pin, and how many projects could
/// not be counted.
#[derive(Debug, Default, PartialEq, Eq)]
struct ToolchainCensus {
    /// Pinned toolchain to number of projects.
    pinned: BTreeMap<String, usize>,
    /// Projects whose root has no (or an empty) `lean-toolchain`.
    unpinned: usize,
    /// Projects whose path no longer exists. They are left out of the
    /// counts so a stale index entry is not reported as a toolchain problem.
    missing_paths: usize,
}

impl ToolchainCensus {
    fn of(projects: &[Project]) -> Self {
        let mut census = Self::default();
        for project in projects {
            let root = project.expanded_path();
            if !root.exists() {
                census.missing_paths += 1;
                continue;
            }
            match read_toolchain(&root) {
                Some(toolchain) => *census.pinned.entry(toolchain).or_insert(0) += 1,
                None => census.unpinned += 1,
            }
        }
        census
    }
}

fn list_toolchains(census: &ToolchainCensus) {
    let mut rows: Vec<Vec<String>> = census
        .pinned
        .iter()
        .map(|(toolchain, count)| vec![toolchain.clone(), count.to_string()])
        .collect();
    if census.unpinned > 0 {
        rows.push(vec![NO_TOOLCHAIN.to_string(), census.unpinned.to_string()]);
    }
    print_table(&["TOOLCHAIN", "PROJECTS"], &rows);
    print_missing_note(census.missing_paths);
}

fn check_toolchains(census: &ToolchainCensus, installed: Option<&BTreeSet<String>>) {
    print_table(
        &["TOOLCHAIN", "PROJECTS", "STATUS"],
        &check_rows(census, installed),
    );
    if installed.is_none() {
        println!("elan could not be run; installed toolchains are unknown.");
    }
    print_missing_note(census.missing_paths);
}

fn check_rows(census: &ToolchainCensus, installed: Option<&BTreeSet<String>>) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = census
        .pinned
        .iter()
        .map(|(toolchain, count)| {
            let status = match installed {
                None => "unknown",
                Some(set) if set.contains(toolchain) => "installed",
                Some(_) => "not installed",
            };
            vec![toolchain.clone(), count.to_string(), status.to_string()]
        })
        .collect();
    if census.unpinned > 0 {
        rows.push(vec![
            NO_TOOLCHAIN.to_string(),
            census.unpinned.to_string(),
            "-".to_string(),
        ]);
    }
    rows
}

fn print_missing_note(missing_paths: usize) {
    if missing_paths > 0 {
        println!(
            "Skipped {missing_paths} project(s) whose path no longer exists (see `leanmgr doctor`)."
        );
    }
}

/// Installed toolchains according to `elan toolchain list`, or `None`
/// when elan is unavailable.
fn installed_toolchains() -> Option<BTreeSet<String>> {
    let output = stdout("elan", &["toolchain", "list"]).ok().flatten()?;
    Some(parse_elan_toolchains(&output))
}

/// Parse `elan toolchain list` output. Each line starts with the toolchain
/// name, optionally followed by an annotation such as `(default)`.
fn parse_elan_toolchains(output: &str) -> BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("leanmgr-toolchain-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn project_at(name: &str, path: &str) -> Project {
        Project {
            name: name.to_string(),
            path: path.to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn census_separates_missing_paths_from_unpinned_projects() {
        let pinned = tmp("pinned");
        fs::write(pinned.join("lean-toolchain"), b"leanprover/lean4:v4.9.0\n").unwrap();
        let unpinned = tmp("unpinned");
        let projects = vec![
            project_at("pinned", &pinned.display().to_string()),
            project_at("unpinned", &unpinned.display().to_string()),
            project_at("gone", "/nonexistent/leanmgr-toolchain-gone"),
        ];

        let census = ToolchainCensus::of(&projects);

        assert_eq!(
            census.pinned,
            BTreeMap::from([("leanprover/lean4:v4.9.0".to_string(), 1)])
        );
        assert_eq!(census.unpinned, 1);
        assert_eq!(census.missing_paths, 1);
        fs::remove_dir_all(&pinned).unwrap();
        fs::remove_dir_all(&unpinned).unwrap();
    }

    #[test]
    fn check_rows_report_install_status() {
        let census = ToolchainCensus {
            pinned: BTreeMap::from([
                ("leanprover/lean4:v4.8.0".to_string(), 2),
                ("leanprover/lean4:v4.9.0".to_string(), 1),
            ]),
            unpinned: 1,
            missing_paths: 0,
        };
        let installed = BTreeSet::from(["leanprover/lean4:v4.9.0".to_string()]);

        let rows = check_rows(&census, Some(&installed));
        let statuses: Vec<&str> = rows.iter().map(|row| row[2].as_str()).collect();
        assert_eq!(statuses, vec!["not installed", "installed", "-"]);
        assert_eq!(rows[2][0], NO_TOOLCHAIN);

        let unknown = check_rows(&census, None);
        assert_eq!(unknown[0][2], "unknown");
    }

    #[test]
    fn parses_elan_list_with_annotations() {
        let output = "leanprover/lean4:v4.9.0 (default)\nleanprover/lean4:v4.8.0\n\n";
        assert_eq!(
            parse_elan_toolchains(output),
            BTreeSet::from([
                "leanprover/lean4:v4.8.0".to_string(),
                "leanprover/lean4:v4.9.0".to_string(),
            ])
        );
    }
}
