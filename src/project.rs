//! Project model and project index commands.

use crate::cli::{AddArgs, ListArgs, RemoveArgs};
use crate::config::{load_config, save_config};
use crate::output::{print_json, print_table};
use crate::paths::{display_path, expand_tilde, normalize_existing_or_join, now_string};
use crate::size::project_size;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A configured Lean project.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Project {
    /// Human-readable unique name.
    pub name: String,
    /// Stored project path.
    pub path: String,
    /// User-defined tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional description.
    #[serde(default)]
    pub description: Option<String>,
    /// Timestamp string when the project was added.
    #[serde(default)]
    pub added_at: Option<String>,
    /// Timestamp string when the project was last seen.
    #[serde(default)]
    pub last_seen_at: Option<String>,
}

impl Project {
    /// Return the expanded project path.
    pub fn expanded_path(&self) -> PathBuf {
        expand_tilde(&self.path)
    }

    /// Return true when the project has a tag.
    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|item| item == tag)
    }
}

/// Add a project to the config.
pub fn add_project(args: AddArgs) -> Result<()> {
    let path = normalize_existing_or_join(&expand_tilde(&args.path))?;
    validate_lake_project(&path)?;

    let mut config = load_config()?;
    let name = args.name.unwrap_or_else(|| {
        path.file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string())
    });

    if config.projects.iter().any(|project| project.name == name) {
        bail!("project name already exists: {name}");
    }

    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", path.display()))?;
    for project in &config.projects {
        if project.expanded_path().canonicalize().ok() == Some(canonical.clone()) {
            bail!("project path is already indexed: {}", path.display());
        }
    }

    let mut tags = args.tags;
    tags.sort();
    tags.dedup();
    let now = now_string();
    config.projects.push(Project {
        name: name.clone(),
        path: display_path(&path),
        tags,
        description: args.description,
        added_at: Some(now.clone()),
        last_seen_at: Some(now),
    });
    save_config(&config)?;
    println!("Added {name}");
    Ok(())
}

/// Remove a project record without deleting source files.
pub fn remove_project(args: RemoveArgs) -> Result<()> {
    let mut config = load_config()?;
    let before = config.projects.len();
    config
        .projects
        .retain(|project| !matches_project(project, &args.project));
    if config.projects.len() == before {
        bail!("project not found: {}", args.project);
    }
    save_config(&config)?;
    println!("Removed {}", args.project);
    Ok(())
}

/// List configured projects.
pub fn list_projects(args: ListArgs) -> Result<()> {
    let config = load_config()?;
    let projects = filter_by_tag(&config.projects, args.tag.as_deref());
    if args.json {
        return print_json(&projects);
    }

    let mut rows = Vec::new();
    for project in projects {
        let size = project_size(project)?;
        rows.push(vec![
            project.name.clone(),
            project.path.clone(),
            crate::output::format_bytes(size.total),
            project.tags.join(","),
        ]);
    }
    print_table(&["NAME", "PATH", "SIZE", "TAGS"], &rows);
    Ok(())
}

/// Return projects matching a selector.
pub fn select_projects(
    project_selector: Option<&str>,
    tag: Option<&str>,
    all: bool,
) -> Result<Vec<Project>> {
    crate::paths::ensure_selector(project_selector.is_some(), tag.is_some(), all)?;
    let config = load_config()?;
    let selected = if let Some(selector) = project_selector {
        let matched: Vec<Project> = config
            .projects
            .into_iter()
            .filter(|project| matches_project(project, selector))
            .collect();
        if matched.is_empty() {
            bail!("project not found: {selector}");
        }
        matched
    } else if let Some(tag) = tag {
        config
            .projects
            .into_iter()
            .filter(|project| project.has_tag(tag))
            .collect()
    } else {
        config.projects
    };
    Ok(selected)
}

/// Validate that a path contains a Lake file.
pub fn validate_lake_project(path: &Path) -> Result<()> {
    if path.join("lakefile.toml").exists() || path.join("lakefile.lean").exists() {
        return Ok(());
    }
    bail!(
        "{} does not contain lakefile.toml or lakefile.lean",
        path.display()
    );
}

/// Return true when a selector matches a project name or path.
pub fn matches_project(project: &Project, selector: &str) -> bool {
    if project.name == selector || project.path == selector {
        return true;
    }
    let selector_path = expand_tilde(selector);
    project.expanded_path() == selector_path
}

/// Filter projects by optional tag.
pub fn filter_by_tag<'a>(projects: &'a [Project], tag: Option<&str>) -> Vec<&'a Project> {
    projects
        .iter()
        .filter(|project| tag.is_none_or(|tag| project.has_tag(tag)))
        .collect()
}
