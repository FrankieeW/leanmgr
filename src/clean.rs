//! Safe `.lake` cleanup planning and execution.

use crate::cli::{CleanArgs, CleanLevel};
use crate::output::{format_bytes, print_table};
use crate::project::{Project, select_projects};
use crate::size::dir_size;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// One planned removal target.
#[derive(Clone, Debug, Serialize)]
pub struct CleanTarget {
    /// Project name.
    pub project: String,
    /// Path to remove.
    pub path: PathBuf,
    /// Reclaimable bytes.
    pub bytes: u64,
}

/// Run the clean command.
pub fn clean_command(args: CleanArgs) -> Result<()> {
    let projects = select_projects(args.project.as_deref(), args.tag.as_deref(), args.all)?;
    let targets = build_clean_plan(&projects, args.level)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    print_clean_plan(&targets);
    println!("Total reclaimable: {}", format_bytes(total));

    if args.dry_run {
        return Ok(());
    }
    if targets.is_empty() {
        return Ok(());
    }
    if !args.force && !confirm_delete(total)? {
        println!("Aborted.");
        return Ok(());
    }

    execute_targets(&targets)?;
    Ok(())
}

/// Remove a planned set of targets, printing each removal.
pub fn execute_targets(targets: &[CleanTarget]) -> Result<()> {
    for target in targets {
        remove_target(target)?;
    }
    Ok(())
}

/// Build cleanup targets for selected projects.
pub fn build_clean_plan(projects: &[Project], level: CleanLevel) -> Result<Vec<CleanTarget>> {
    let mut targets = Vec::new();
    for project in projects {
        let root = project.expanded_path();
        let lake = root.join(".lake");
        match level {
            CleanLevel::Soft => push_existing(project, &lake, lake.join("build"), &mut targets)?,
            CleanLevel::DepsBuild => {
                let packages = lake.join("packages");
                if packages.exists() {
                    for entry in fs::read_dir(&packages)
                        .with_context(|| format!("failed to read {}", packages.display()))?
                    {
                        let entry = entry?;
                        if entry.file_type()?.is_dir() {
                            push_existing(
                                project,
                                &lake,
                                entry.path().join(".lake").join("build"),
                                &mut targets,
                            )?;
                        }
                    }
                }
            }
            CleanLevel::Hard => push_existing(project, &lake, lake.clone(), &mut targets)?,
        }
    }
    Ok(targets)
}

fn push_existing(
    project: &Project,
    lake: &Path,
    target: PathBuf,
    targets: &mut Vec<CleanTarget>,
) -> Result<()> {
    if !target.exists() {
        return Ok(());
    }
    validate_lake_containment(lake, &target)?;
    targets.push(CleanTarget {
        project: project.name.clone(),
        bytes: dir_size(&target)?,
        path: target,
    });
    Ok(())
}

fn print_clean_plan(targets: &[CleanTarget]) {
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|target| {
            vec![
                target.project.clone(),
                target.path.display().to_string(),
                format_bytes(target.bytes),
            ]
        })
        .collect();
    print_table(&["PROJECT", "WOULD REMOVE", "SIZE"], &rows);
}

pub(crate) fn confirm_delete(total: u64) -> Result<bool> {
    print!("Delete {}? [y/N] ", format_bytes(total));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(matches!(input.trim(), "y" | "Y" | "yes" | "YES"))
}

fn remove_target(target: &CleanTarget) -> Result<()> {
    if target.path.is_dir() {
        fs::remove_dir_all(&target.path)
            .with_context(|| format!("failed to remove {}", target.path.display()))?;
    } else if target.path.exists() {
        fs::remove_file(&target.path)
            .with_context(|| format!("failed to remove {}", target.path.display()))?;
    }
    println!("Removed {}", target.path.display());
    Ok(())
}

fn validate_lake_containment(lake: &Path, target: &Path) -> Result<()> {
    if fs::symlink_metadata(target)
        .with_context(|| format!("failed to stat {}", target.display()))?
        .file_type()
        .is_symlink()
    {
        bail!("refusing to remove symlink {}", target.display());
    }
    let canonical_lake = lake
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", lake.display()))?;
    let canonical_target = target
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", target.display()))?;
    if !canonical_target.starts_with(&canonical_lake) {
        bail!(
            "refusing to remove {} because it is outside {}",
            target.display(),
            lake.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn hard_clean_targets_only_project_lake() {
        let root = test_dir("hard_clean_targets_only_project_lake");
        fs::create_dir_all(root.join(".lake/build")).unwrap();
        fs::write(root.join(".lake/build/file"), b"data").unwrap();

        let project = Project {
            name: "demo".to_string(),
            path: root.display().to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        };

        let targets = build_clean_plan(&[project], CleanLevel::Hard).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].path, root.join(".lake"));
        assert!(targets[0].bytes > 0);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn execute_targets_removes_paths() {
        let root = test_dir("execute_targets_removes_paths");
        fs::create_dir_all(root.join(".lake/build")).unwrap();
        fs::write(root.join(".lake/build/file"), b"data").unwrap();

        let target = CleanTarget {
            project: "demo".to_string(),
            path: root.join(".lake"),
            bytes: 4,
        };
        execute_targets(&[target]).unwrap();
        assert!(!root.join(".lake").exists());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deps_build_targets_nested_package_builds() {
        let root = test_dir("deps_build_targets_nested_package_builds");
        fs::create_dir_all(root.join(".lake/packages/mathlib/.lake/build")).unwrap();
        fs::write(
            root.join(".lake/packages/mathlib/.lake/build/file"),
            b"data",
        )
        .unwrap();

        let project = Project {
            name: "demo".to_string(),
            path: root.display().to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        };

        let targets = build_clean_plan(&[project], CleanLevel::DepsBuild).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets[0].path,
            root.join(".lake/packages/mathlib/.lake/build")
        );

        fs::remove_dir_all(root).unwrap();
    }

    fn test_dir(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }
}
