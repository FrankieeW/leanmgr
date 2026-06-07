//! Core library for the LeanMgr command line tool.

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, InteractArgs, TagCommands, WorktreeCommands};

pub mod ai;
pub mod clean;
pub mod cli;
pub mod config;
pub mod discovery;
pub mod doctor;
pub mod gc;
pub mod gitignore;
pub mod interactive;
pub mod output;
pub mod paths;
pub mod process;
pub mod project;
pub mod recover;
pub mod restore;
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
    }
}
