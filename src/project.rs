//! Project model and project index commands.

use crate::cli::{AddArgs, ListArgs, RemoveArgs};
use crate::config::{load_config, save_config};
use crate::lake::has_lakefile;
use crate::output::{format_age, format_bytes, print_json, print_table};
use crate::paths::{display_path, expand_tilde, normalize_existing_or_join, now_secs, now_string};
use crate::prompt::confirm;
use crate::size::{ProjectSize, project_size};
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
    /// Timestamp string of the project's most recent commit, if known.
    ///
    /// Accepts the legacy `last_commit_at` spelling as a serde alias for
    /// backward compatibility with older `config.json` files.
    #[serde(default, alias = "last_commit_at")]
    pub last_committed_at: Option<String>,
    /// Cached `.lake` size information for fast list output.
    #[serde(default)]
    pub size_cache: Option<SizeCache>,
}

/// Cached `.lake` size information.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SizeCache {
    /// Bytes under `.lake`.
    pub lake: u64,
    /// Bytes under `.lake/build`.
    pub build: u64,
    /// Bytes under `.lake/packages`.
    pub packages: u64,
    /// Total bytes counted for the project.
    pub total: u64,
    /// Timestamp string when this entry was computed.
    pub computed_at: String,
}

impl SizeCache {
    /// Build a cache entry from a fresh size calculation.
    pub fn from_project_size(size: &ProjectSize) -> Self {
        Self {
            lake: size.lake,
            build: size.build,
            packages: size.packages,
            total: size.total,
            computed_at: now_string(),
        }
    }
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

    /// Whether the project directory is present, gone, or cannot be checked.
    ///
    /// `Path::exists` reports `false` on permission errors too, which would
    /// make an unreadable project look deleted; only a definite "not found"
    /// counts as missing.
    pub fn path_state(&self) -> PathState {
        match self.expanded_path().try_exists() {
            Ok(true) => PathState::Present,
            Ok(false) => PathState::Missing,
            Err(_) => PathState::Unreadable,
        }
    }
}

/// Result of checking a project's path on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathState {
    /// The path exists.
    Present,
    /// The path definitely does not exist.
    Missing,
    /// The path could not be checked, e.g. a parent denies access.
    Unreadable,
}

impl PathState {
    /// Label used in the `list` STATUS column.
    pub fn label(self) -> &'static str {
        match self {
            Self::Present => "ok",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
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
        last_seen_at: Some(now.clone()),
        last_committed_at: Some(now),
        size_cache: None,
    });
    save_config(&config)?;
    println!("Added {name}");
    Ok(())
}

/// Remove project records without deleting source files.
pub fn remove_project(args: RemoveArgs) -> Result<()> {
    if args.missing {
        return remove_missing_projects(args.dry_run, args.force);
    }
    let selector = args
        .project
        .as_deref()
        .context("select a project or pass --missing")?;
    let mut config = load_config()?;
    let matched: Vec<&Project> = config
        .projects
        .iter()
        .filter(|project| matches_project(project, selector))
        .collect();
    if matched.is_empty() {
        bail!("project not found: {selector}");
    }
    if args.dry_run {
        for project in matched {
            println!("Would remove {}  {}", project.name, project.path);
        }
        return Ok(());
    }
    config
        .projects
        .retain(|project| !matches_project(project, selector));
    save_config(&config)?;
    println!("Removed {selector}");
    Ok(())
}

/// Drop every index entry whose path no longer exists.
///
/// Only `config.json` changes; nothing on disk is deleted. The list is
/// confirmed first because a project on an unmounted drive also looks
/// missing, and removing it would lose its tags and description.
fn remove_missing_projects(dry_run: bool, force: bool) -> Result<()> {
    let mut config = load_config()?;
    let missing = missing_entries(&config.projects);
    if missing.is_empty() {
        println!("Every indexed project path exists; nothing to remove.");
        return Ok(());
    }

    println!("Projects whose path no longer exists:");
    let rows: Vec<Vec<String>> = missing
        .iter()
        .map(|(name, path)| vec![name.clone(), path.clone()])
        .collect();
    print_table(&["NAME", "PATH"], &rows);
    if dry_run {
        println!("Dry run: the index was not changed.");
        return Ok(());
    }
    if !force {
        println!(
            "Paths on an unmounted drive also look missing. Source directories are never touched."
        );
        if !confirm(&format!(
            "Remove {} project(s) from the index?",
            missing.len()
        ))? {
            println!("Aborted.");
            return Ok(());
        }
    }

    config.projects.retain(|project| {
        !missing
            .iter()
            .any(|(name, path)| *name == project.name && *path == project.path)
    });
    save_config(&config)?;
    println!("Removed {} project(s) from the index.", missing.len());
    Ok(())
}

/// Return `(name, path)` for each project whose path does not exist.
fn missing_entries(projects: &[Project]) -> Vec<(String, String)> {
    projects
        .iter()
        .filter(|project| project.path_state() == PathState::Missing)
        .map(|project| (project.name.clone(), project.path.clone()))
        .collect()
}

/// List configured projects.
pub fn list_projects(args: ListArgs) -> Result<()> {
    let mut config = load_config()?;
    if args.sizes {
        refresh_size_cache(&mut config.projects, args.tag.as_deref())?;
        save_config(&config)?;
    }

    let projects = filter_by_tag(&config.projects, args.tag.as_deref());
    if args.json {
        return print_json(&projects);
    }

    let now = now_secs();
    let rows: Vec<Vec<String>> = projects
        .iter()
        .map(|project| list_row(project, now))
        .collect();
    print_table(
        &["NAME", "STATUS", "SIZE", "MEASURED", "TAGS", "PATH"],
        &rows,
    );
    let missing = rows
        .iter()
        .filter(|row| row[1] == PathState::Missing.label())
        .count();
    if missing > 0 {
        println!(
            "{missing} project path(s) no longer exist; `leanmgr remove --missing` drops them from the index."
        );
    }
    Ok(())
}

fn list_row(project: &Project, now: u64) -> Vec<String> {
    let (size, measured) = cached_size_cells(project, now);
    vec![
        project.name.clone(),
        project.path_state().label().to_string(),
        size,
        measured,
        project.tags.join(","),
        project.path.clone(),
    ]
}

fn refresh_size_cache(projects: &mut [Project], tag: Option<&str>) -> Result<()> {
    for project in projects {
        if tag.is_some_and(|tag| !project.has_tag(tag)) {
            continue;
        }
        let size = project_size(project)?;
        project.size_cache = Some(SizeCache::from_project_size(&size));
    }
    Ok(())
}

fn cached_size_cells(project: &Project, now: u64) -> (String, String) {
    project
        .size_cache
        .as_ref()
        .map(|cache| {
            (
                format_bytes(cache.total),
                format_age(&cache.computed_at, now),
            )
        })
        .unwrap_or_else(|| ("unknown".to_string(), "never".to_string()))
}

/// Return projects matching a selector.
pub fn select_projects(
    project_selector: Option<&str>,
    tag: Option<&str>,
    all: bool,
) -> Result<Vec<Project>> {
    ensure_selector(project_selector.is_some(), tag.is_some(), all)?;
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

/// Ensure that a required project selector mode is valid.
///
/// Project selection is a 3-way exclusive choice between a project
/// name/path, a `--tag` filter, and `--all`. Exactly one must be set.
fn ensure_selector(project: bool, tag: bool, all: bool) -> Result<()> {
    let count = [project, tag, all]
        .into_iter()
        .filter(|value| *value)
        .count();
    if count == 1 {
        return Ok(());
    }
    bail!("select exactly one of project, --tag, or --all");
}

/// Validate that a path contains a Lake file.
pub fn validate_lake_project(path: &Path) -> Result<()> {
    if has_lakefile(path) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_size_cells_show_unknown_without_cache() {
        let project = project_with_cache(None);
        assert_eq!(
            cached_size_cells(&project, 0),
            ("unknown".to_string(), "never".to_string())
        );
    }

    #[test]
    fn cached_size_cells_use_cached_values() {
        let project = project_with_cache(Some(SizeCache {
            lake: 2048,
            build: 1024,
            packages: 512,
            total: 2048,
            computed_at: "123".to_string(),
        }));
        assert_eq!(
            cached_size_cells(&project, 123 + 2 * 86_400),
            ("2.0 KiB".to_string(), "2d ago".to_string())
        );
    }

    #[test]
    fn list_row_flags_missing_paths_and_puts_path_last() {
        let mut project = project_with_cache(None);
        project.path = "/nonexistent/leanmgr-list-row".to_string();
        project.tags = vec!["a".to_string(), "b".to_string()];
        let row = list_row(&project, 0);
        assert_eq!(
            row,
            vec![
                "demo".to_string(),
                PathState::Missing.label().to_string(),
                "unknown".to_string(),
                "never".to_string(),
                "a,b".to_string(),
                "/nonexistent/leanmgr-list-row".to_string(),
            ]
        );
    }

    #[test]
    fn missing_entries_only_reports_absent_paths() {
        let present = std::env::temp_dir();
        let mut kept = project_with_cache(None);
        kept.name = "kept".to_string();
        kept.path = present.display().to_string();
        let mut gone = project_with_cache(None);
        gone.name = "gone".to_string();
        gone.path = "/nonexistent/leanmgr-missing-entry".to_string();

        let missing = missing_entries(&[kept, gone]);
        assert_eq!(
            missing,
            vec![(
                "gone".to_string(),
                "/nonexistent/leanmgr-missing-entry".to_string()
            )]
        );
    }

    /// A project under a directory that denies access is not "missing":
    /// `remove --missing` must never drop it.
    #[cfg(unix)]
    #[test]
    fn unreadable_paths_are_not_missing() {
        use std::os::unix::fs::PermissionsExt;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let locked = std::env::temp_dir().join(format!("leanmgr-unreadable-{nonce}"));
        std::fs::create_dir_all(locked.join("child")).unwrap();
        let mut project = project_with_cache(None);
        project.path = locked.join("child").display().to_string();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let state = project.path_state();
        let missing = missing_entries(std::slice::from_ref(&project));

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&locked).unwrap();
        // Root ignores permission bits and sees the directory as present.
        assert!(matches!(state, PathState::Unreadable | PathState::Present));
        assert!(missing.is_empty());
    }

    fn project_with_cache(size_cache: Option<SizeCache>) -> Project {
        Project {
            name: "demo".to_string(),
            path: "/tmp/demo".to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache,
        }
    }
}
