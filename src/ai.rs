//! AI-agent context output and skill installation helpers.

use crate::cli::{
    AiCommands, AiContextArgs, AiOutputFormat, AiSkillAddArgs, AiSkillCommands, AiSkillShowArgs,
    AiSkillTarget,
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

    match args.format {
        AiOutputFormat::Json => print_json(&context),
        AiOutputFormat::Markdown => {
            print_markdown_context(&context, "LeanMgr AI Context");
            Ok(())
        }
        AiOutputFormat::Codex => {
            print_agent_context(&context, "Codex Task Context");
            Ok(())
        }
        AiOutputFormat::Claude => {
            print_agent_context(&context, "Claude Code Task Context");
            Ok(())
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
    let _format = args.format;
    let skill = read_skill_file()?;
    print!("{skill}");
    Ok(())
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

fn print_markdown_context(context: &AiProjectContext, title: &str) {
    println!("# {title}");
    println!();
    println!("Projects: {}", context.project_count);
    println!();
    println!("## Constraints");
    for constraint in &context.constraints {
        println!("- {constraint}");
    }
    println!();
    println!("## Projects");
    for project in &context.projects {
        println!(
            "- {}: {} ({})",
            project.name,
            project.path,
            format_bytes(project.lake_bytes)
        );
    }
    println!();
    println!("## Recommended Workflow");
    for step in &context.recommended_workflow {
        println!("- {step}");
    }
}

fn print_agent_context(context: &AiProjectContext, title: &str) {
    println!("# {title}");
    println!();
    println!("You are working with LeanMgr, a CLI for managing disposable `.lake` cache state.");
    println!("Honor these constraints before proposing or executing cleanup.");
    println!();
    print_markdown_context(context, "Project State");
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
}
