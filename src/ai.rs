//! AI-agent context output and skill installation helpers.

use crate::cli::{
    AiCommands, AiContextArgs, AiOutputFormat, AiSkillAddArgs, AiSkillCommands, AiSkillFormat,
    AiSkillShowArgs, AiSkillTarget,
};
use crate::config::load_config;
use crate::output::{format_bytes, print_json};
use crate::project::{Project, filter_by_tag};
use crate::size::{ProjectSize, project_size};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;

const LEANMGR_SKILL_NAME: &str = "leanmgr-cache-manager";
const DEFAULT_SKILL_SOURCE: &str = "github:FrankieeW/agent-skills/leanmgr-cache-manager";
const CODEX_SKILL_URL: &str =
    "https://github.com/FrankieeW/agent-skills/tree/main/codex/leanmgr-cache-manager";
const CLAUDE_SKILL_URL: &str =
    "https://github.com/FrankieeW/agent-skills/tree/main/claude-code/leanmgr-cache-manager";

/// Run an AI command.
pub fn ai_command(command: AiCommands) -> Result<()> {
    match command {
        AiCommands::Context(args) => context_command(args),
        AiCommands::Skill { command } => match command {
            AiSkillCommands::List => skill_list_command(),
            AiSkillCommands::Show(args) => skill_show_command(args),
            AiSkillCommands::Add(args) => skill_add_command(args),
        },
    }
}

#[derive(Debug, Serialize)]
struct AiProjectContext {
    project_count: usize,
    projects: Vec<AiProject>,
    constraints: Vec<&'static str>,
    recommended_workflow: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct AiProject {
    name: String,
    path: String,
    tags: Vec<String>,
    lake_bytes: u64,
    build_bytes: u64,
    packages_bytes: u64,
}

fn context_command(args: AiContextArgs) -> Result<()> {
    let config = load_config()?;
    let selected = filter_by_tag(&config.projects, args.tag.as_deref());
    let context = build_context(&selected)?;
    let mut stdout = io::stdout();

    match args.format {
        AiOutputFormat::Json => print_json(&context),
        AiOutputFormat::Markdown => {
            print_markdown_context(&context, "LeanMgr AI Context", &mut stdout)
        }
        AiOutputFormat::Codex => print_agent_context(&context, "Codex Task Context", &mut stdout),
        AiOutputFormat::Claude => {
            print_agent_context(&context, "Claude Code Task Context", &mut stdout)
        }
    }
}

fn skill_list_command() -> Result<()> {
    println!("{LEANMGR_SKILL_NAME}");
    Ok(())
}

fn skill_show_command(args: AiSkillShowArgs) -> Result<()> {
    if args.name != LEANMGR_SKILL_NAME {
        bail!("unknown skill: {}", args.name);
    }
    let skill = read_skill_file()?;
    let adapted = adapt_skill_for(&skill, args.format);
    print!("{adapted}");
    Ok(())
}

/// Adapt the canonical SKILL.md body for the requested agent format.
///
/// The skill body is single-source: it lives in the agent-skills repo and
/// is shared between Codex and Claude Code. The CLI still honors
/// `--format` so a user (or a codex/claude sub-shell) can pin a
/// recipient-specific contract header on the printed body. Three cases:
/// - the body's first non-empty line is the matching header → unchanged
/// - the body's first non-empty line is a different `# ...` heading →
///   replace that heading with the expected one
/// - the body has no leading heading → prepend the expected one
pub(crate) fn adapt_skill_for(body: &str, fmt: AiSkillFormat) -> String {
    let expected = skill_header(fmt);
    let mut lines = body.lines();
    let first = lines.next().unwrap_or("");
    match first {
        line if line == expected => body.to_string(),
        line if line.starts_with("# ") => replace_first_heading(body, expected),
        _ => format!("{expected}\n\n{body}"),
    }
}

fn skill_header(fmt: AiSkillFormat) -> &'static str {
    match fmt {
        AiSkillFormat::Codex => "# Codex Task Contract",
        AiSkillFormat::Claude => "# Claude Code Task Context",
    }
}

fn replace_first_heading(body: &str, expected: &str) -> String {
    // Find the end of the first line; everything after it is preserved verbatim.
    let Some(newline_at) = body.find('\n') else {
        return expected.to_string();
    };
    let mut rest = &body[newline_at + 1..];
    // Skip a single blank line that often follows a heading, so the
    // replacement doesn't leave two blank lines in a row.
    if let Some(stripped) = rest.strip_prefix("\n") {
        rest = stripped;
    }
    format!("{expected}\n\n{rest}")
}

fn skill_add_command(args: AiSkillAddArgs) -> Result<()> {
    let source = normalize_skill_source(args.source.as_deref());
    if args.dry_run {
        println!("Would run: npx skills add {source}");
        print_fallbacks(args.target);
        return Ok(());
    }

    let status = Command::new("npx")
        .args(["skills", "add", &source])
        .status();
    match status {
        Ok(status) if status.success() => {
            println!("Installed skill with npx skills add {source}");
            Ok(())
        }
        Ok(status) => {
            println!("npx skills add failed with status {status}");
            fallback_skill_install(args)
        }
        Err(error) => {
            println!("npx skills add could not start: {error}");
            fallback_skill_install(args)
        }
    }
}

fn build_context(projects: &[&Project]) -> Result<AiProjectContext> {
    let mut ai_projects = Vec::new();
    for project in projects {
        let size = project_size(project)?;
        ai_projects.push(project_context(project, size));
    }
    Ok(AiProjectContext {
        project_count: ai_projects.len(),
        projects: ai_projects,
        constraints: vec![
            "Do not modify Lean source files for cache cleanup tasks.",
            "Do not modify Git history.",
            "Treat .lake as disposable cache.",
            "Use lake, elan, and git as authoritative tools.",
            "All destructive cleanup should be dry-run first.",
        ],
        recommended_workflow: vec![
            "Run leanmgr size or doctor to identify candidates.",
            "Run leanmgr clean with --dry-run before deletion.",
            "Use leanmgr gitignore to ensure .lake/ is ignored.",
            "Use leanmgr restore to call lake exe cache get when returning to a project.",
        ],
    })
}

fn normalize_skill_source(source: Option<&str>) -> String {
    match source {
        None => DEFAULT_SKILL_SOURCE.to_string(),
        Some(value) if value == LEANMGR_SKILL_NAME => DEFAULT_SKILL_SOURCE.to_string(),
        Some(value) => value.to_string(),
    }
}

fn project_context(project: &Project, size: ProjectSize) -> AiProject {
    AiProject {
        name: project.name.clone(),
        path: project.path.clone(),
        tags: project.tags.clone(),
        lake_bytes: size.lake,
        build_bytes: size.build,
        packages_bytes: size.packages,
    }
}

fn print_markdown_context<W: Write>(
    context: &AiProjectContext,
    title: &str,
    output: &mut W,
) -> Result<()> {
    writeln!(output, "# {title}")?;
    writeln!(output)?;
    writeln!(output, "Projects: {}", context.project_count)?;
    writeln!(output)?;
    writeln!(output, "## Constraints")?;
    for constraint in &context.constraints {
        writeln!(output, "- {constraint}")?;
    }
    writeln!(output)?;
    writeln!(output, "## Projects")?;
    for project in &context.projects {
        writeln!(
            output,
            "- {}: {} ({})",
            project.name,
            project.path,
            format_bytes(project.lake_bytes)
        )?;
    }
    writeln!(output)?;
    writeln!(output, "## Recommended Workflow")?;
    for step in &context.recommended_workflow {
        writeln!(output, "- {step}")?;
    }
    Ok(())
}

fn print_agent_context<W: Write>(
    context: &AiProjectContext,
    title: &str,
    output: &mut W,
) -> Result<()> {
    writeln!(output, "# {title}")?;
    writeln!(output)?;
    writeln!(
        output,
        "You are working with LeanMgr, a CLI for managing disposable `.lake` cache state."
    )?;
    writeln!(
        output,
        "Honor these constraints before proposing or executing cleanup."
    )?;
    writeln!(output)?;
    print_markdown_context(context, "Project State", output)
}

fn fallback_skill_install(args: AiSkillAddArgs) -> Result<()> {
    print_fallbacks(args.target);
    if !args.yes && !confirm("Open/use one of the fallback skill addresses manually?")? {
        println!("Skipped fallback install.");
        return Ok(());
    }

    println!(
        "Use one of the fallback addresses above, or set LEANMGR_SKILL_PATH to a local SKILL.md."
    );
    Ok(())
}

fn print_fallbacks(target: AiSkillTarget) {
    println!("Fallback candidates:");
    if matches!(target, AiSkillTarget::Auto | AiSkillTarget::Codex) {
        println!("  codex: {CODEX_SKILL_URL}");
        println!("    local: leanmgr ai skill show {LEANMGR_SKILL_NAME} --format codex");
    }
    if matches!(target, AiSkillTarget::Auto | AiSkillTarget::Claude) {
        println!("  claude-code: {CLAUDE_SKILL_URL}");
        println!("    local: leanmgr ai skill show {LEANMGR_SKILL_NAME} --format claude");
    }
}

fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read confirmation")?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}

fn read_skill_file() -> Result<String> {
    let candidates = skill_path_candidates();
    for path in &candidates {
        if path.exists() {
            return fs::read_to_string(path)
                .with_context(|| format!("failed to read {}", path.display()));
        }
    }
    bail!(
        "skill file not found. Install with `leanmgr ai skill add`, set LEANMGR_SKILL_PATH, or use {DEFAULT_SKILL_SOURCE}"
    )
}

fn skill_path_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = env::var_os("LEANMGR_SKILL_PATH") {
        paths.push(PathBuf::from(path));
    }
    paths.push(PathBuf::from("SKILL.md"));
    paths.push(PathBuf::from(LEANMGR_SKILL_NAME).join("SKILL.md"));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-ai-{name}-{nonce}"))
    }

    fn project_with_lake(name: &str) -> Project {
        let root = tmp(name);
        fs::create_dir_all(root.join(".lake/build")).unwrap();
        fs::write(root.join(".lake/build/file"), b"abcd").unwrap();
        Project {
            name: name.to_string(),
            path: root.display().to_string(),
            tags: vec!["msc".to_string()],
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn skill_candidates_are_portable() {
        let candidates = skill_path_candidates();
        assert!(
            candidates
                .iter()
                .any(|path| path == &PathBuf::from("SKILL.md"))
        );
        assert!(
            candidates
                .iter()
                .any(|path| path == &PathBuf::from("leanmgr-cache-manager").join("SKILL.md"))
        );
    }

    #[test]
    fn default_skill_source_points_to_future_repo() {
        assert_eq!(
            normalize_skill_source(None),
            "github:FrankieeW/agent-skills/leanmgr-cache-manager"
        );
        assert_eq!(
            normalize_skill_source(Some("leanmgr-cache-manager")),
            "github:FrankieeW/agent-skills/leanmgr-cache-manager"
        );
    }

    /// `build_context` must always emit the constraints and the recommended
    /// workflow. These are the contract AI agents rely on; a regression
    /// here would silently change how downstream agents reason about
    /// LeanMgr.
    #[test]
    fn build_context_includes_constraints_and_workflow() {
        let project = project_with_lake("ctx");
        let context = build_context(&[&project]).unwrap();
        assert_eq!(context.project_count, 1);
        assert!(
            context
                .constraints
                .iter()
                .any(|line| line.contains("dry-run"))
        );
        assert!(
            context
                .recommended_workflow
                .iter()
                .any(|line| line.contains("leanmgr clean"))
        );
        fs::remove_dir_all(project.expanded_path()).ok();
    }

    /// `print_markdown_context` must put the configured title on the first
    /// line and mention each project by name.
    #[test]
    fn print_markdown_context_renders_title_and_projects() {
        let project = project_with_lake("md");
        let context = build_context(&[&project]).unwrap();
        let mut out = Vec::new();
        print_markdown_context(&context, "Test Title", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("# Test Title"));
        assert!(text.contains("md"));
        fs::remove_dir_all(project.expanded_path()).ok();
    }

    /// `print_agent_context` for Codex must use the Codex-specific title
    /// header so agents can detect the contract.
    #[test]
    fn print_agent_context_codex_uses_codex_title() {
        let project = project_with_lake("codex");
        let context = build_context(&[&project]).unwrap();
        let mut out = Vec::new();
        print_agent_context(&context, "Codex Task Context", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Codex Task Context"));
        assert!(text.contains(".lake"));
        fs::remove_dir_all(project.expanded_path()).ok();
    }

    /// `print_agent_context` for Claude must use the Claude-specific title
    /// header.
    #[test]
    fn print_agent_context_claude_uses_claude_title() {
        let project = project_with_lake("claude");
        let context = build_context(&[&project]).unwrap();
        let mut out = Vec::new();
        print_agent_context(&context, "Claude Code Task Context", &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Claude Code Task Context"));
        fs::remove_dir_all(project.expanded_path()).ok();
    }

    /// `adapt_skill_for` is a no-op when the body's first line already
    /// matches the expected header. This is the "the agent-skills repo
    /// shipped the right header" case.
    #[test]
    fn adapt_skill_for_keeps_matching_header() {
        let body = "# Codex Task Contract\n\nbody\n";
        let out = adapt_skill_for(body, AiSkillFormat::Codex);
        assert_eq!(out, body);
    }

    /// `adapt_skill_for` replaces a wrong leading heading with the
    /// expected one, then keeps the rest of the body verbatim.
    #[test]
    fn adapt_skill_for_replaces_wrong_header() {
        let body = "# Codex Task Contract\n\nbody line\n";
        let out = adapt_skill_for(body, AiSkillFormat::Claude);
        assert!(out.starts_with("# Claude Code Task Context\n"));
        assert!(out.contains("body line"));
    }

    /// `adapt_skill_for` prepends the expected header when the body has
    /// no leading heading at all. This covers a fresh SKILL.md that
    /// starts with prose.
    #[test]
    fn adapt_skill_for_prepends_missing_header() {
        let body = "Some prose with no heading.\nMore body.\n";
        let out = adapt_skill_for(body, AiSkillFormat::Codex);
        assert!(out.starts_with("# Codex Task Contract\n\nSome prose"));
    }
}
