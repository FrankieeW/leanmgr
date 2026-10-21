//! Command line interface definitions.

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

/// Manage disposable `.lake` cache state across many Lean 4 projects.
#[derive(Debug, Parser)]
#[command(name = "leanmgr", version, about)]
pub struct Cli {
    /// Command to run.
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Top-level command set.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// AI-agent oriented context and skill helpers.
    Ai {
        /// AI command to run.
        #[command(subcommand)]
        command: AiCommands,
    },
    /// Create the default configuration file.
    Init,
    /// Add a Lake project to the project index.
    Add(AddArgs),
    /// Remove project records without deleting source files.
    Remove(RemoveArgs),
    /// List indexed projects.
    List(ListArgs),
    /// Recursively find Lake projects under a directory.
    Scan(ScanArgs),
    /// Report `.lake` disk usage.
    Size(SizeArgs),
    /// Diagnose cache, toolchain, and project health.
    Doctor(DoctorArgs),
    /// Start an interactive cache-management assistant.
    Interact(InteractArgs),
    /// Ensure selected projects ignore `.lake/`.
    Gitignore(GitignoreArgs),
    /// Remove `.lake`-scoped cache paths.
    Clean(CleanArgs),
    /// Reclaim `.lake` cache across projects by policy, skipping unrecoverable caches.
    Gc(GcArgs),
    /// Run `lake exe cache get` for selected projects.
    Restore(RestoreArgs),
    /// Manage project tags.
    Tag {
        /// Tag command to run.
        #[command(subcommand)]
        command: TagCommands,
    },
    /// Audit Lean toolchains across projects.
    Toolchain {
        /// Toolchain command to run.
        #[command(subcommand)]
        command: ToolchainCommands,
    },
    /// Audit Git worktrees across projects.
    Worktree {
        /// Worktree command to run.
        #[command(subcommand)]
        command: WorktreeCommands,
    },
    /// Print a shell completion script to stdout.
    Completions(CompletionsArgs),
}

/// Arguments for `leanmgr completions`.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub shell: Shell,
}

/// AI subcommands.
#[derive(Debug, Subcommand)]
pub enum AiCommands {
    /// Emit project context in an AI-friendly format.
    Context(AiContextArgs),
    /// Manage LeanMgr AI skills.
    Skill {
        /// Skill command to run.
        #[command(subcommand)]
        command: AiSkillCommands,
    },
}

/// Arguments for `leanmgr ai context`.
#[derive(Debug, Args)]
pub struct AiContextArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = AiOutputFormat::Markdown)]
    pub format: AiOutputFormat,
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
}

/// AI-oriented output formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AiOutputFormat {
    /// Markdown optimized for coding agents.
    Markdown,
    /// JSON for machine ingestion.
    Json,
    /// Codex-flavored markdown with explicit task contract.
    Codex,
    /// Claude Code-flavored markdown with explicit task contract.
    Claude,
}

/// AI skill subcommands.
#[derive(Debug, Subcommand)]
pub enum AiSkillCommands {
    /// List known skill names.
    List,
    /// Show an installed or local skill definition.
    Show(AiSkillShowArgs),
    /// Install a skill using `npx skills add` first, then fallback prompts.
    Add(AiSkillAddArgs),
}

/// Arguments for `leanmgr ai skill show`.
#[derive(Debug, Args)]
pub struct AiSkillShowArgs {
    /// Skill name.
    pub name: String,
    /// Output adapter. The underlying skill is canonical and single-source.
    #[arg(long, value_enum, default_value_t = AiSkillFormat::Codex)]
    pub format: AiSkillFormat,
}

/// Arguments for `leanmgr ai skill add`.
#[derive(Debug, Args)]
pub struct AiSkillAddArgs {
    /// Skill name or source accepted by `npx skills add`.
    pub source: Option<String>,
    /// Preferred target when npx install fails.
    #[arg(long, value_enum, default_value_t = AiSkillTarget::Auto)]
    pub target: AiSkillTarget,
    /// Print commands and fallback choices without executing.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip fallback confirmation prompt.
    #[arg(long)]
    pub yes: bool,
}

/// Built-in skill format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AiSkillFormat {
    /// Codex SKILL.md format.
    Codex,
    /// Claude Code skill markdown format.
    Claude,
}

/// AI skill installation target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum AiSkillTarget {
    /// Show both supported fallback targets.
    Auto,
    /// Codex skill target.
    Codex,
    /// Claude Code skill target.
    Claude,
}

/// Arguments for `leanmgr add`.
#[derive(Debug, Args)]
pub struct AddArgs {
    /// Path to a Lake project.
    pub path: String,
    /// Project name to store. Defaults to the directory name.
    #[arg(long)]
    pub name: Option<String>,
    /// Tags to attach to the project.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Optional project description.
    #[arg(long)]
    pub description: Option<String>,
}

/// Arguments for `leanmgr remove`.
#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Project name or path.
    #[arg(required_unless_present = "missing", conflicts_with = "missing")]
    pub project: Option<String>,
    /// Remove every project whose path no longer exists.
    #[arg(long)]
    pub missing: bool,
    /// Print what would be removed without changing the index.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip the confirmation prompt for `--missing`.
    #[arg(long)]
    pub force: bool,
}

/// Arguments for `leanmgr list`.
#[derive(Debug, Args)]
pub struct ListArgs {
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Recompute `.lake` sizes and update the size cache.
    #[arg(long)]
    pub sizes: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `leanmgr scan`.
#[derive(Debug, Args)]
pub struct ScanArgs {
    /// Root directory to scan.
    #[arg(required_unless_present = "from_elan", conflicts_with = "from_elan")]
    pub root: Option<String>,
    /// Discover the projects elan has seen Lean used in, wherever they are.
    #[arg(long)]
    pub from_elan: bool,
    /// Add discovered projects without prompting.
    #[arg(long)]
    pub yes: bool,
    /// Emit JSON and do not modify config.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `leanmgr size`.
#[derive(Debug, Args)]
pub struct SizeArgs {
    /// Optional project name or path.
    pub project: Option<String>,
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Include all projects.
    ///
    /// Cannot be combined with a project selector or `--tag`: those would
    /// silently win, leaving `--all` a flag that does nothing.
    #[arg(long, conflicts_with_all = ["project", "tag"])]
    pub all: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `leanmgr doctor`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Consider projects unused after this many days since `.lake` modification.
    #[arg(long, default_value_t = 90)]
    pub unused_days: u64,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `leanmgr interact`.
#[derive(Debug, Args)]
pub struct InteractArgs {
    /// Consider projects unused after this many days since `.lake` modification.
    #[arg(long, default_value_t = 90)]
    pub unused_days: u64,
    /// Filter the assistant scope by tag.
    #[arg(long)]
    pub tag: Option<String>,
}

/// Arguments for `leanmgr gitignore`.
#[derive(Debug, Args)]
pub struct GitignoreArgs {
    /// Optional project name or path.
    pub project: Option<String>,
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Select all projects.
    #[arg(long)]
    pub all: bool,
    /// Print planned changes without writing files.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for `leanmgr clean`.
#[derive(Debug, Args)]
pub struct CleanArgs {
    /// Optional project name or path.
    pub project: Option<String>,
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Select all projects.
    #[arg(long)]
    pub all: bool,
    /// Cleanup level.
    #[arg(long, value_enum)]
    pub level: CleanLevel,
    /// Print planned removals without deleting.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip confirmation prompt.
    #[arg(long)]
    pub force: bool,
}

/// Cleanup levels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CleanLevel {
    /// Remove `<project>/.lake/build`.
    Soft,
    /// Remove `<project>/.lake/packages/*/.lake/build`.
    DepsBuild,
    /// Remove `<project>/.lake`.
    Hard,
}

/// Arguments for `leanmgr gc`.
#[derive(Debug, Args)]
pub struct GcArgs {
    /// Only consider projects whose `.lake` is older than this many days.
    #[arg(long, required_unless_present = "target")]
    pub unused_days: Option<u64>,
    /// Reclaim until at least this much space is freed (e.g. 20GiB),
    /// largest caches first. Combine with --unused-days to keep recently
    /// used caches out of reach.
    #[arg(long)]
    pub target: Option<String>,
    /// Filter by tag. Default scope is all projects.
    #[arg(long)]
    pub tag: Option<String>,
    /// Cleanup level.
    #[arg(long, value_enum, default_value_t = CleanLevel::Hard)]
    pub level: CleanLevel,
    /// Also delete caches that are not recoverable.
    #[arg(long)]
    pub include_unrecoverable: bool,
    /// Print the plan without deleting.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip confirmation prompt.
    #[arg(long)]
    pub force: bool,
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `leanmgr restore`.
#[derive(Debug, Args)]
pub struct RestoreArgs {
    /// Optional project name or path.
    pub project: Option<String>,
    /// Filter by tag.
    #[arg(long)]
    pub tag: Option<String>,
    /// Select all projects.
    #[arg(long)]
    pub all: bool,
    /// Print which projects would be restored or skipped without running Lake.
    #[arg(long)]
    pub dry_run: bool,
}

/// Tag subcommands.
#[derive(Debug, Subcommand)]
pub enum TagCommands {
    /// Add a tag to a project.
    Add(TagEditArgs),
    /// Remove a tag from a project.
    Remove(TagEditArgs),
    /// List all tags.
    List,
}

/// Arguments for tag add/remove.
#[derive(Debug, Args)]
pub struct TagEditArgs {
    /// Project name or path.
    pub project: String,
    /// Tag to add or remove.
    pub tag: String,
}

/// Toolchain subcommands.
#[derive(Debug, Subcommand)]
pub enum ToolchainCommands {
    /// List toolchains referenced by indexed projects.
    List,
    /// Check referenced toolchains against installed elan toolchains.
    Check,
}

/// Worktree subcommands.
#[derive(Debug, Subcommand)]
pub enum WorktreeCommands {
    /// List worktrees for indexed projects.
    List(WorktreeArgs),
    /// Report broken worktrees.
    Doctor(WorktreeArgs),
    /// Run `git worktree prune`.
    Prune(WorktreePruneArgs),
}

/// Arguments for worktree read-only commands.
#[derive(Debug, Args)]
pub struct WorktreeArgs {
    /// Emit JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for `worktree prune`.
#[derive(Debug, Args)]
pub struct WorktreePruneArgs {
    /// Print what Git would prune.
    #[arg(long)]
    pub dry_run: bool,
    /// Skip confirmation prompt.
    #[arg(long)]
    pub force: bool,
}
