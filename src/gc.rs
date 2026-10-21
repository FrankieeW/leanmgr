//! Policy-driven, recoverability-checked cache cleanup.

use crate::clean::{
    CleanTarget, build_clean_plan_from_scan, confirm_delete, execute_targets, print_clean_plan,
};
use crate::cli::{CleanLevel, GcArgs};
use crate::config::load_config;
use crate::doctor::{is_unused_mtime, unused_cutoff};
use crate::output::{format_bytes, parse_bytes, print_json};
use crate::project::Project;
use crate::recover::{Recoverability, assess};
use crate::size::scan_lake;
use anyhow::{Result, bail};
use serde::Serialize;

/// Resolved gc options. At least one of `unused_days` and `target` is set;
/// together they mean "free `target` bytes from caches unused for
/// `unused_days`", so a size budget can never reach an active project.
pub struct GcOptions {
    /// Only consider projects whose `.lake` mtime is older than N days.
    pub unused_days: Option<u64>,
    /// Stop once at least this many bytes are selected, largest first.
    pub target: Option<u64>,
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
    let cutoff = opts.unused_days.map(unused_cutoff);

    for project in projects {
        // One traversal of the `.lake` tree answers both "is this cache
        // stale?" and "how much would deleting it free?".
        let scan = scan_lake(project);

        if let Some(cutoff) = cutoff
            && !is_unused_mtime(scan.size.latest_modified, cutoff)
        {
            continue;
        }

        let project_targets = build_clean_plan_from_scan(project, &scan, opts.level)?;
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
    match opts.target {
        None => {
            for candidate in candidates {
                targets.extend(candidate.targets);
            }
        }
        Some(budget) => {
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

/// The selection policy as reported by `gc --json`: `{"unused_days": N}`,
/// `{"target_bytes": N}`, or both when combined.
#[derive(Serialize)]
struct GcModeJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    unused_days: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_bytes: Option<u64>,
}

#[derive(Serialize)]
struct GcReport<'a> {
    mode: GcModeJson,
    targets: &'a [CleanTarget],
    skipped: &'a [GcSkip],
    total_bytes: u64,
    executed: bool,
}

impl From<&GcOptions> for GcModeJson {
    fn from(opts: &GcOptions) -> Self {
        Self {
            unused_days: opts.unused_days,
            target_bytes: opts.target,
        }
    }
}

/// Run the gc command: select by policy, report, confirm, and delete.
pub fn gc_command(args: GcArgs) -> Result<()> {
    if args.unused_days.is_none() && args.target.is_none() {
        bail!("select --unused-days, --target, or both");
    }
    let opts = GcOptions {
        unused_days: args.unused_days,
        target: args.target.as_deref().map(parse_bytes).transpose()?,
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

    if args.json {
        // JSON mode never prompts (scripts can't answer interactively);
        // --force is the only way to execute from JSON.
        let executed = args.force && !args.dry_run && !targets.is_empty();
        if executed {
            execute_targets(&targets, true)?;
        }
        return print_json(&GcReport {
            mode: GcModeJson::from(&opts),
            targets: &targets,
            skipped: &skipped,
            total_bytes: total,
            executed,
        });
    }

    // The whole plan is printed before the prompt so the answer is informed.
    print_gc_plan(&targets, &skipped, total);
    if args.dry_run || targets.is_empty() {
        return Ok(());
    }
    if !args.force && !confirm_delete(total)? {
        println!("Aborted.");
        return Ok(());
    }
    execute_targets(&targets, false)
}

fn print_gc_plan(targets: &[CleanTarget], skipped: &[GcSkip], total: u64) {
    if targets.is_empty() {
        println!("Nothing to reclaim.");
    } else {
        print_clean_plan(targets);
    }
    if !skipped.is_empty() {
        println!("\nSkipped (unrecoverable, use --include-unrecoverable to force):");
        for skip in skipped {
            println!("  {}  {}", skip.project, skip.reason);
        }
    }
    if !targets.is_empty() {
        println!("Total reclaimable: {}", format_bytes(total));
    }
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
            unused_days: Some(0),
            target: None,
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
            unused_days: Some(0),
            target: None,
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
            unused_days: None,
            target: Some(5000),
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

    #[test]
    fn unused_days_keeps_a_budget_away_from_active_caches() {
        let root = tmp("gc_budget_unused");
        let active = mk_project(&root, "active", 4096, true);
        let stale = mk_project(&root, "stale", 2048, true);
        // A write "in the future" keeps `active` newer than any cutoff.
        let in_an_hour = SystemTime::now() + std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(active.expanded_path().join(".lake/blob"))
            .unwrap()
            .set_modified(in_an_hour)
            .unwrap();
        let projects = [active, stale];
        let budget_only = GcOptions {
            unused_days: None,
            target: Some(1000),
            level: CleanLevel::Hard,
            include_unrecoverable: false,
        };
        let (targets, _) = plan_gc(&projects, &budget_only).unwrap();
        assert_eq!(
            targets[0].project, "active",
            "largest first without a filter"
        );

        let protected = GcOptions {
            unused_days: Some(0),
            ..budget_only
        };
        let (targets, _) = plan_gc(&projects, &protected).unwrap();
        let names: Vec<&str> = targets
            .iter()
            .map(|target| target.project.as_str())
            .collect();
        assert_eq!(names, vec!["stale"]);
        fs::remove_dir_all(&root).unwrap();
    }
}
