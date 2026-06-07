use anyhow::Result;
use clap::Parser;
use leanmgr::cli::{Cli, Commands, TagCommands, WorktreeCommands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Ai { command } => leanmgr::ai::ai_command(command),
        Commands::Init => leanmgr::config::init_config(),
        Commands::Add(args) => leanmgr::project::add_project(args),
        Commands::Remove(args) => leanmgr::project::remove_project(args),
        Commands::List(args) => leanmgr::project::list_projects(args),
        Commands::Scan(args) => leanmgr::discovery::scan_command(args),
        Commands::Size(args) => leanmgr::size::size_command(args),
        Commands::Doctor(args) => leanmgr::doctor::doctor_command(args),
        Commands::Gitignore(args) => leanmgr::gitignore::gitignore_command(args),
        Commands::Clean(args) => leanmgr::clean::clean_command(args),
        Commands::Restore(args) => leanmgr::restore::restore_command(args),
        Commands::Tag { command } => match command {
            TagCommands::Add(args) => leanmgr::tags::add_tag(args),
            TagCommands::Remove(args) => leanmgr::tags::remove_tag(args),
            TagCommands::List => leanmgr::tags::list_tags(),
        },
        Commands::Toolchain { command } => leanmgr::toolchain::toolchain_command(command),
        Commands::Worktree { command } => match command {
            WorktreeCommands::List(args) => leanmgr::worktree::list_worktrees(args),
            WorktreeCommands::Doctor(args) => leanmgr::worktree::doctor_worktrees(args),
            WorktreeCommands::Prune(args) => leanmgr::worktree::prune_worktrees(args),
        },
    }
}
