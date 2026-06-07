//! Tag management commands.

use crate::cli::TagEditArgs;
use crate::config::{load_config, save_config};
use crate::output::print_table;
use crate::project::matches_project;
use anyhow::{Result, bail};
use std::collections::BTreeSet;

/// Add a tag to a project.
pub fn add_tag(args: TagEditArgs) -> Result<()> {
    let mut config = load_config()?;
    let project = config
        .projects
        .iter_mut()
        .find(|project| matches_project(project, &args.project))
        .ok_or_else(|| anyhow::anyhow!("project not found: {}", args.project))?;
    if !project.tags.contains(&args.tag) {
        project.tags.push(args.tag.clone());
        project.tags.sort();
    }
    save_config(&config)?;
    println!("Added tag {}", args.tag);
    Ok(())
}

/// Remove a tag from a project.
pub fn remove_tag(args: TagEditArgs) -> Result<()> {
    let mut config = load_config()?;
    let project = config
        .projects
        .iter_mut()
        .find(|project| matches_project(project, &args.project))
        .ok_or_else(|| anyhow::anyhow!("project not found: {}", args.project))?;
    let before = project.tags.len();
    project.tags.retain(|tag| tag != &args.tag);
    if project.tags.len() == before {
        bail!("tag not found: {}", args.tag);
    }
    save_config(&config)?;
    println!("Removed tag {}", args.tag);
    Ok(())
}

/// List all tags.
pub fn list_tags() -> Result<()> {
    let config = load_config()?;
    let mut tags = BTreeSet::new();
    for project in config.projects {
        for tag in project.tags {
            tags.insert(tag);
        }
    }
    let rows: Vec<Vec<String>> = tags.into_iter().map(|tag| vec![tag]).collect();
    print_table(&["TAG"], &rows);
    Ok(())
}
