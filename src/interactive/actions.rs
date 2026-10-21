//! The five action renderers reached from the dashboard. Each one is
//! either a single-shot plan (`print_gc_dry_run`) or a small interactive
//! picker (`print_clean_dry_run`, `print_restore_command`).
//!
//! All actions share a common pattern: print a section heading, render a
//! table or text block, and end with the exact CLI invocation the user
//! can run to execute the plan.

use super::helpers::{
    clean_level_arg, parse_clean_level, print_section_to, print_table_to, prompt, shell_arg,
    write_name_list,
};
use crate::clean::build_clean_plan;
use crate::cli::CleanLevel;
use crate::doctor::DoctorReport;
use crate::gc::{GcOptions, plan_gc};
use crate::output::format_bytes;
use crate::project::Project;
use crate::restore::{RestoreStep, plan_restore};
use anyhow::{Context, Result, bail};
use std::io::{BufRead, Write};

/// Action 1: list indexed projects with their cached sizes.
pub(super) fn print_project_list<W: Write>(projects: &[Project], output: &mut W) -> Result<()> {
    print_section_to(output, "Projects")?;
    let rows: Vec<Vec<String>> = projects
        .iter()
        .enumerate()
        .map(|(index, project)| {
            let size = project
                .size_cache
                .as_ref()
                .map(|cache| format_bytes(cache.total))
                .unwrap_or_else(|| "unknown".to_string());
            vec![
                (index + 1).to_string(),
                project.name.clone(),
                project.tags.join(","),
                size,
                project.path.clone(),
            ]
        })
        .collect();
    print_table_to(output, &["#", "NAME", "TAGS", "CACHED SIZE", "PATH"], &rows)?;
    Ok(())
}

/// Action 2: doctor details. Mirrors the dedicated `leanmgr doctor`
/// output but writes to the session writer.
pub(super) fn print_doctor_summary<W: Write>(
    doctor: &DoctorReport,
    unused_days: u64,
    output: &mut W,
) -> Result<()> {
    print_section_to(output, "Doctor details")?;
    writeln!(
        output,
        "Potential reclaim: {}",
        format_bytes(doctor.potential_hard_reclaim)
    )?;
    if let Some(project) = &doctor.largest_project {
        writeln!(
            output,
            "Largest project: {} {}",
            project.project,
            format_bytes(project.total)
        )?;
    }
    if !doctor.unused_projects.is_empty() {
        writeln!(output, "Unused > {unused_days} days:")?;
        for project in &doctor.unused_projects {
            writeln!(
                output,
                "  {} {}",
                project.project,
                format_bytes(project.total)
            )?;
        }
    }
    write_name_list(output, "Missing paths", &doctor.missing_paths)?;
    write_name_list(output, "Missing Lake files", &doctor.missing_lake_files)?;
    if !doctor.toolchains.is_empty() {
        writeln!(output, "Toolchains:")?;
        for (toolchain, count) in &doctor.toolchains {
            writeln!(output, "  {toolchain} {count}")?;
        }
    }
    Ok(())
}

/// Action 3: gc dry-run over the whole scope, mirroring
/// `leanmgr gc --unused-days <N> --dry-run`.
pub(super) fn print_gc_dry_run<W: Write>(
    projects: &[Project],
    unused_days: u64,
    tag: Option<&str>,
    output: &mut W,
) -> Result<()> {
    let opts = GcOptions {
        unused_days: Some(unused_days),
        target: None,
        level: CleanLevel::Hard,
        include_unrecoverable: false,
    };
    let (targets, skipped) = plan_gc(projects, &opts)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    print_section_to(output, "GC dry-run")?;
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|target| {
            vec![
                target.project.clone(),
                format_bytes(target.bytes),
                target.path.display().to_string(),
            ]
        })
        .collect();
    print_table_to(output, &["PROJECT", "SIZE", "WOULD REMOVE"], &rows)?;
    writeln!(output, "Total reclaimable: {}", format_bytes(total))?;
    if !skipped.is_empty() {
        writeln!(output, "Skipped as unrecoverable:")?;
        for skip in skipped {
            writeln!(output, "  {} {}", skip.project, skip.reason)?;
        }
    }
    // The suggested command must cover exactly the scope that was previewed.
    // Without `--tag` it would reach every indexed project, which is a
    // different — and much larger — set than the one on screen.
    let scope = match tag {
        Some(tag) => format!(" --tag {}", shell_arg(tag)),
        None => String::new(),
    };
    writeln!(
        output,
        "To execute after review: leanmgr gc --unused-days {unused_days} --level hard{scope}"
    )?;
    Ok(())
}

/// Action 4: pick one project, pick a level, print the plan and the
/// exact `leanmgr clean` invocation to execute it.
pub(super) fn print_clean_dry_run<R: BufRead, W: Write>(
    projects: &[Project],
    input: &mut R,
    output: &mut W,
) -> Result<()> {
    let project = choose_project(projects, input, output)?;
    let level = choose_clean_level(input, output)?;
    let targets = build_clean_plan(std::slice::from_ref(project), level)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    print_section_to(output, "Clean dry-run")?;
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|target| {
            vec![
                target.project.clone(),
                format_bytes(target.bytes),
                target.path.display().to_string(),
            ]
        })
        .collect();
    print_table_to(output, &["PROJECT", "SIZE", "WOULD REMOVE"], &rows)?;
    writeln!(output, "Total reclaimable: {}", format_bytes(total))?;
    writeln!(
        output,
        "To execute after review: leanmgr clean {} --level {}",
        shell_arg(&project.name),
        clean_level_arg(level)
    )?;
    Ok(())
}

/// Action 5: pick one project and print a `leanmgr restore` command the
/// user can run to refill the cache from Lake.
pub(super) fn print_restore_command<R: BufRead, W: Write>(
    projects: &[Project],
    input: &mut R,
    output: &mut W,
) -> Result<()> {
    let project = choose_project(projects, input, output)?;
    print_section_to(output, "Restore command")?;
    match plan_restore(project) {
        RestoreStep::CacheGet => {
            writeln!(
                output,
                "Run this when you want Lake to restore cache artifacts:"
            )?;
            writeln!(output, "leanmgr restore {}", shell_arg(&project.name))?;
        }
        RestoreStep::Skip(reason) => {
            writeln!(
                output,
                "leanmgr restore would skip {}: {reason}.",
                project.name
            )?;
        }
    }
    Ok(())
}

/// List indexed projects and ask the user to pick one by index or name.
pub(super) fn choose_project<'a, R: BufRead, W: Write>(
    projects: &'a [Project],
    input: &mut R,
    output: &mut W,
) -> Result<&'a Project> {
    print_project_list(projects, output)?;
    let selector = prompt("Project number or name", input, output)?
        .context("no project selected (end of input)")?;
    let trimmed = selector.trim();
    if let Ok(index) = trimmed.parse::<usize>() {
        if index == 0 {
            bail!("project index out of range: {index}");
        }
        return projects
            .get(index - 1)
            .with_context(|| format!("project index out of range: {index}"));
    }
    projects
        .iter()
        .find(|project| project.name == trimmed)
        .with_context(|| format!("project not found: {trimmed}"))
}

/// Prompt the user for a clean level and parse the answer.
pub(super) fn choose_clean_level<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
) -> Result<CleanLevel> {
    writeln!(output, "Clean levels: soft, deps-build, hard")?;
    let level =
        prompt("Clean level", input, output)?.context("no clean level selected (end of input)")?;
    parse_clean_level(level.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;

    fn project(name: &str, tags: Vec<&str>) -> Project {
        Project {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            tags: tags.into_iter().map(str::to_string).collect(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    /// The command the assistant suggests after a gc preview must cover
    /// exactly the scope that was previewed. Without `--tag` it would
    /// reach every indexed project, including ones the user never saw.
    #[test]
    fn gc_suggestion_keeps_the_tag_scope() {
        let projects = vec![project("demo", vec!["archived"])];
        let mut out: Vec<u8> = Vec::new();
        print_gc_dry_run(&projects, 90, Some("archived"), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("leanmgr gc --unused-days 90 --level hard --tag archived"),
            "suggested command must carry the tag scope, got: {text}"
        );
    }

    #[test]
    fn gc_suggestion_without_a_tag_has_no_tag_flag() {
        let projects = vec![project("demo", vec![])];
        let mut out: Vec<u8> = Vec::new();
        print_gc_dry_run(&projects, 90, None, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("leanmgr gc --unused-days 90 --level hard"));
        assert!(!text.contains("--tag"));
    }

    /// A tag with shell metacharacters must be quoted, because the
    /// suggestion is meant to be pasted into a shell.
    #[test]
    fn gc_suggestion_quotes_the_tag() {
        let projects = vec![project("demo", vec!["two words"])];
        let mut out: Vec<u8> = Vec::new();
        print_gc_dry_run(&projects, 90, Some("two words"), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("--tag 'two words'"), "got: {text}");
    }
}
