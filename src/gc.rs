//! Policy-driven, recoverability-checked cache cleanup.

use crate::clean::{CleanTarget, build_clean_plan, confirm_delete, execute_targets};
use crate::cli::{CleanLevel, GcArgs};
use crate::config::load_config;
use crate::doctor::{is_unused, unused_cutoff};
use crate::output::{format_bytes, parse_bytes, print_json, print_table};
use crate::project::Project;
use crate::recover::{Recoverability, assess};
use anyhow::{Result, bail};
use serde::Serialize;
use std::time::SystemTime;

/// How gc selects which projects to clean.
pub enum GcMode {
    /// Projects whose `.lake` mtime is older than N days.
    UnusedDays(u64),
    /// Reclaim until at least this many bytes are freed (largest first).
    Target(u64),
}

/// Resolved gc options.
pub struct GcOptions {
    pub mode: GcMode,
    pub level: CleanLevel,
    pub include_unrecoverable: bool,
}

/// A project excluded from the plan because it is not recoverable.
#[derive(Clone, Debug, Serialize)]
pub struct GcSkip {
    pub project: String,
    pub reason: String,
}

/// Pure selection. Reads sizes/mtime/recoverability from disk but never mutates.
pub fn plan_gc(projects: &[Project], opts: &GcOptions) -> Result<(Vec<CleanTarget>, Vec<GcSkip>)> {
    struct Candidate {
        bytes: u64,
        targets: Vec<CleanTarget>,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut skipped: Vec<GcSkip> = Vec::new();

    let cutoff: Option<SystemTime> = match opts.mode {
        GcMode::UnusedDays(days) => Some(unused_cutoff(days)),
        GcMode::Target(_) => None,
    };

    for project in projects {
        if let Some(cutoff) = cutoff {
            let lake = project.expanded_path().join(".lake");
            if !is_unused(&lake, cutoff)? {
                continue;
            }
        }

        let project_targets = build_clean_plan(std::slice::from_ref(project), opts.level)?;
        let bytes: u64 = project_targets.iter().map(|target| target.bytes).sum();
        if bytes == 0 {
            continue;
        }

        if let Recoverability::Unrecoverable(reason) = assess(project)
            && !opts.include_unrecoverable
        {
            skipped.push(GcSkip {
                project: project.name.clone(),
                reason,
            });
            continue;
        }

        candidates.push(Candidate {
            bytes,
            targets: project_targets,
        });
    }

    let mut targets = Vec::new();
    match opts.mode {
        GcMode::UnusedDays(_) => {
            for candidate in candidates {
                targets.extend(candidate.targets);
            }
        }
        GcMode::Target(budget) => {
            candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.bytes));
            let mut freed = 0u64;
            for candidate in candidates {
                if freed >= budget {
                    break;
                }
                freed += candidate.bytes;
                targets.extend(candidate.targets);
            }
        }
    }

    Ok((targets, skipped))
}

#[derive(Serialize)]
#[serde(untagged)]
enum GcModeJson {
    UnusedDays { unused_days: u64 },
    Target { target_bytes: u64 },
}

#[derive(Serialize)]
struct GcReport<'a> {
    mode: GcModeJson,
    targets: &'a [CleanTarget],
    skipped: &'a [GcSkip],
    total_bytes: u64,
    executed: bool,
}

/// Run the gc command: select by policy, report, confirm, and delete.
pub fn gc_command(args: GcArgs) -> Result<()> {
    let mode = match (args.unused_days, args.target.as_deref()) {
        (Some(_), Some(_)) | (None, None) => {
            bail!("select exactly one of --unused-days or --target")
        }
        (Some(days), None) => GcMode::UnusedDays(days),
        (None, Some(value)) => GcMode::Target(parse_bytes(value)?),
    };
    let opts = GcOptions {
        mode,
        level: args.level,
        include_unrecoverable: args.include_unrecoverable,
    };

    let config = load_config()?;
    let scope: Vec<Project> = match args.tag.as_deref() {
        Some(tag) => config
            .projects
            .into_iter()
            .filter(|project| project.has_tag(tag))
            .collect(),
        None => config.projects,
    };

    let (targets, skipped) = plan_gc(&scope, &opts)?;
    let total: u64 = targets.iter().map(|target| target.bytes).sum();

    // Decide whether to delete. JSON mode never prompts (scripts can't answer
    // interactively); --force is the only way to execute from JSON.
    let will_execute = if args.dry_run || targets.is_empty() {
        false
    } else if args.force {
        true
    } else if args.json {
        false
    } else {
        confirm_delete(total)?
    };

    if args.json {
        if will_execute {
            execute_targets(&targets)?;
        }
        let mode = match opts.mode {
            GcMode::UnusedDays(days) => GcModeJson::UnusedDays { unused_days: days },
            GcMode::Target(bytes) => GcModeJson::Target {
                target_bytes: bytes,
            },
        };
        return print_json(&GcReport {
            mode,
            targets: &targets,
            skipped: &skipped,
            total_bytes: total,
            executed: will_execute,
        });
    }

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

    if !skipped.is_empty() {
        println!("\nSkipped (unrecoverable, use --include-unrecoverable to force):");
        for skip in &skipped {
            println!("  {}  {}", skip.project, skip.reason);
        }
    }

    println!("Total reclaimable: {}", format_bytes(total));

    if !will_execute {
        if !args.dry_run && !targets.is_empty() {
            println!("Aborted.");
        }
        return Ok(());
    }
    execute_targets(&targets)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }

    fn mk_project(root: &Path, name: &str, lake_bytes: usize, recoverable: bool) -> Project {
        let proj = root.join(name);
        fs::create_dir_all(proj.join(".lake")).unwrap();
        fs::write(proj.join(".lake/blob"), vec![b'x'; lake_bytes]).unwrap();
        if recoverable {
            fs::write(proj.join("lakefile.toml"), b"").unwrap();
            fs::write(proj.join("lake-manifest.json"), b"{}").unwrap();
            fs::write(proj.join("lean-toolchain"), b"leanprover/lean4:v4.0.0\n").unwrap();
        }
        Project {
            name: name.to_string(),
            path: proj.display().to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn age_mode_skips_unrecoverable_by_default() {
        let root = tmp("gc_age_skip");
        let ok = mk_project(&root, "ok", 1024, true);
        let bad = mk_project(&root, "bad", 1024, false);
        let opts = GcOptions {
            mode: GcMode::UnusedDays(0),
            level: CleanLevel::Hard,
            include_unrecoverable: false,
        };
        let (targets, skipped) = plan_gc(&[ok, bad], &opts).unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].project, "ok");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].project, "bad");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn include_unrecoverable_keeps_all() {
        let root = tmp("gc_age_force");
        let ok = mk_project(&root, "ok", 1024, true);
        let bad = mk_project(&root, "bad", 1024, false);
        let opts = GcOptions {
            mode: GcMode::UnusedDays(0),
            level: CleanLevel::Hard,
            include_unrecoverable: true,
        };
        let (targets, skipped) = plan_gc(&[ok, bad], &opts).unwrap();
        assert_eq!(targets.len(), 2);
        assert!(skipped.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn target_mode_picks_largest_until_budget_met() {
        let root = tmp("gc_budget");
        let big = mk_project(&root, "big", 4096, true);
        let mid = mk_project(&root, "mid", 2048, true);
        let small = mk_project(&root, "small", 1024, true);
        let opts = GcOptions {
            mode: GcMode::Target(5000),
            level: CleanLevel::Hard,
            include_unrecoverable: false,
        };
        let (targets, skipped) = plan_gc(&[big, mid, small], &opts).unwrap();
        let names: Vec<String> = targets
            .iter()
            .map(|target| target.project.clone())
            .collect();
        assert_eq!(targets.len(), 2);
        assert!(names.contains(&"big".to_string()));
        assert!(names.contains(&"mid".to_string()));
        assert!(skipped.is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}
