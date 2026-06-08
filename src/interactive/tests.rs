//! Unit tests for the interactive module. Kept in a sibling file so
//! `mod.rs` stays small and the test fixture helper does not pollute
//! the public surface.

use super::helpers::{parse_clean_level, shell_arg};
use super::summary::summarize_fleet;
use crate::cli::CleanLevel;
use crate::project::{Project, SizeCache};

#[test]
fn summarize_fleet_counts_cached_sizes_and_tags() {
    let projects = vec![
        project("one", vec!["active"], Some(1024)),
        project("two", vec!["active", "archive"], None),
    ];
    let summary = summarize_fleet(&projects);

    assert_eq!(summary.project_count, 2);
    assert_eq!(summary.measured_count, 1);
    assert_eq!(summary.cached_total, 1024);
    assert_eq!(summary.tags.get("active"), Some(&2));
    assert_eq!(summary.tags.get("archive"), Some(&1));
}

#[test]
fn summarize_fleet_zero_input_returns_zero_summary() {
    // The empty scope is the "no projects indexed" case the assistant
    // sees before the first `leanmgr add`; the summary it derives must
    // be all zeros / empty, not a panic.
    let summary = summarize_fleet(&[]);
    assert_eq!(summary.project_count, 0);
    assert_eq!(summary.measured_count, 0);
    assert_eq!(summary.cached_total, 0);
    assert!(summary.tags.is_empty());
}

#[test]
fn parse_clean_level_accepts_cli_names() {
    assert_eq!(parse_clean_level("soft").unwrap(), CleanLevel::Soft);
    assert_eq!(
        parse_clean_level("deps-build").unwrap(),
        CleanLevel::DepsBuild
    );
    assert_eq!(
        parse_clean_level("deps_build").unwrap(),
        CleanLevel::DepsBuild
    );
    assert_eq!(parse_clean_level("hard").unwrap(), CleanLevel::Hard);
    assert!(parse_clean_level("deep").is_err());
}

#[test]
fn parse_clean_level_rejects_empty_and_whitespace() {
    // The pipe-mode prompt passes `level.trim()` before this, so the
    // empty case is reachable only when the user submits a literal
    // whitespace-only line. The function still must reject it.
    assert!(parse_clean_level("").is_err());
    assert!(parse_clean_level("   ").is_err());
    assert!(parse_clean_level("HARD ").is_err());
}

#[test]
fn shell_arg_quotes_spaces_and_single_quotes() {
    assert_eq!(shell_arg("demo"), "demo");
    assert_eq!(shell_arg("two words"), "'two words'");
    assert_eq!(shell_arg("bob's"), "'bob'\\''s'");
}

fn project(name: &str, tags: Vec<&str>, total: Option<u64>) -> Project {
    Project {
        name: name.to_string(),
        path: format!("/tmp/{name}"),
        tags: tags.into_iter().map(str::to_string).collect(),
        description: None,
        added_at: None,
        last_seen_at: None,
        last_committed_at: None,
        size_cache: total.map(|total| SizeCache {
            lake: total,
            build: 0,
            packages: 0,
            total,
            computed_at: "1".to_string(),
        }),
    }
}
