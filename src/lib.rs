//! Core library for the LeanMgr command line tool.

use anyhow::Result;
use clap::{CommandFactory, Parser};
use cli::{Cli, Commands, CompletionsArgs, InteractArgs, TagCommands, WorktreeCommands};

pub mod ai;
pub mod clean;
pub mod cli;
pub mod config;
pub mod discovery;
pub mod doctor;
pub mod gc;
pub mod gitignore;
pub mod interactive;
pub mod lake;
pub mod output;
pub mod paths;
pub mod process;
pub mod project;
pub mod prompt;
pub mod recover;
pub mod restore;
pub mod shared_cache;
pub mod size;
pub mod tags;
pub mod toolchain;
pub mod worktree;

/// Parse command-line arguments and dispatch the selected command.
pub fn run_cli() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => interactive::interact_command(InteractArgs {
            unused_days: 90,
            tag: None,
        }),
        Some(Commands::Ai { command }) => ai::ai_command(command),
        Some(Commands::Init) => config::init_config(),
        Some(Commands::Add(args)) => project::add_project(args),
        Some(Commands::Remove(args)) => project::remove_project(args),
        Some(Commands::List(args)) => project::list_projects(args),
        Some(Commands::Scan(args)) => discovery::scan_command(args),
        Some(Commands::Size(args)) => size::size_command(args),
        Some(Commands::Doctor(args)) => doctor::doctor_command(args),
        Some(Commands::Interact(args)) => interactive::interact_command(args),
        Some(Commands::Gitignore(args)) => gitignore::gitignore_command(args),
        Some(Commands::Clean(args)) => clean::clean_command(args),
        Some(Commands::Gc(args)) => gc::gc_command(args),
        Some(Commands::Restore(args)) => restore::restore_command(args),
        Some(Commands::Tag { command }) => match command {
            TagCommands::Add(args) => tags::add_tag(args),
            TagCommands::Remove(args) => tags::remove_tag(args),
            TagCommands::List => tags::list_tags(),
        },
        Some(Commands::Toolchain { command }) => toolchain::toolchain_command(command),
        Some(Commands::Worktree { command }) => match command {
            WorktreeCommands::List(args) => worktree::list_worktrees(args),
            WorktreeCommands::Doctor(args) => worktree::doctor_worktrees(args),
            WorktreeCommands::Prune(args) => worktree::prune_worktrees(args),
        },
        Some(Commands::Completions(args)) => completions_command(args),
    }
}

/// Print a completion script in the requested shell. The script registers
/// under the name the binary was invoked as, so `lm completions zsh`
/// completes `lm` and `leanmgr completions zsh` completes `leanmgr`.
fn completions_command(args: CompletionsArgs) -> Result<()> {
    let mut command = Cli::command();
    let name = completion_name(std::env::args_os().next().as_deref())
        .unwrap_or_else(|| command.get_name().to_string());
    clap_complete::generate(args.shell, &mut command, name, &mut std::io::stdout());
    Ok(())
}

/// `Some("lm")` when invoked through the short `lm` binary; `None` (use
/// the canonical `leanmgr`) otherwise.
fn completion_name(argv0: Option<&std::ffi::OsStr>) -> Option<String> {
    let stem = std::path::Path::new(argv0?).file_stem()?.to_str()?;
    (stem == "lm").then(|| stem.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn completion_name_follows_the_short_binary_only() {
        assert_eq!(
            completion_name(Some(OsStr::new("/usr/local/bin/lm"))),
            Some("lm".to_string())
        );
        assert_eq!(
            completion_name(Some(OsStr::new("C:\\bin\\lm.exe"))).as_deref(),
            if cfg!(windows) { Some("lm") } else { None }
        );
        assert_eq!(completion_name(Some(OsStr::new("leanmgr"))), None);
        assert_eq!(completion_name(Some(OsStr::new("lm-dev"))), None);
        assert_eq!(completion_name(None), None);
    }
}
