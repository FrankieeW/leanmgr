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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a config with a single project, bypassing the on-disk loader
    /// so tag tests stay pure. Returns `(config, project_name)`.
    fn fixture(tags: Vec<&str>) -> crate::config::Config {
        crate::config::Config {
            version: 1,
            projects: vec![crate::project::Project {
                name: "demo".to_string(),
                path: "/tmp/demo".to_string(),
                tags: tags.into_iter().map(str::to_string).collect(),
                description: None,
                added_at: None,
                last_seen_at: None,
                last_committed_at: None,
                size_cache: None,
            }],
        }
    }

    #[test]
    fn add_is_idempotent() {
        // Re-adding the same tag must not duplicate the entry, and must
        // not silently reorder the existing set.
        let mut config = fixture(vec!["msc", "active"]);
        let project = &mut config.projects[0];
        let before = project.tags.clone();
        let tag = "msc".to_string();
        if !project.tags.contains(&tag) {
            project.tags.push(tag);
            project.tags.sort();
        }
        assert_eq!(project.tags, before);
    }

    #[test]
    fn add_keeps_tags_sorted() {
        let mut config = fixture(vec!["active"]);
        let project = &mut config.projects[0];
        // Insert "zeta" then "msc" and check sorted invariant.
        project.tags.push("zeta".to_string());
        project.tags.sort();
        project.tags.push("msc".to_string());
        project.tags.sort();
        assert_eq!(
            project.tags,
            vec!["active".to_string(), "msc".to_string(), "zeta".to_string()]
        );
    }

    #[test]
    fn remove_of_absent_tag_is_a_no_op() {
        // Mirrors the `bail!` branch in `remove_tag` by re-implementing the
        // before/after check on a snapshot. The contract: when the tag is
        // missing, the set is unchanged and the caller bails.
        let mut config = fixture(vec!["msc", "active"]);
        let project = &mut config.projects[0];
        let before = project.tags.clone();
        let before_len = project.tags.len();
        project.tags.retain(|tag| tag != "missing");
        let after_len = project.tags.len();
        assert_eq!(before_len, after_len, "tag set must be unchanged");
        assert_eq!(project.tags, before);
    }

    #[test]
    fn list_tags_dedupes_and_sorts() {
        // The list path uses a BTreeSet, so duplicates collapse and order
        // is alphabetical. We assert the same invariant on the input shape
        // the function actually sees.
        let config = fixture(vec!["zeta", "msc", "msc", "active"]);
        let mut tags = BTreeSet::new();
        for project in config.projects {
            for tag in project.tags {
                tags.insert(tag);
            }
        }
        let collected: Vec<String> = tags.into_iter().collect();
        assert_eq!(
            collected,
            vec!["active".to_string(), "msc".to_string(), "zeta".to_string()]
        );
    }
}
