//! Fleet summary derived from indexed project metadata. Used by the
//! dashboard and the keyboard menu to render counts, totals, and tag
//! histograms.

use crate::project::Project;
use std::collections::BTreeMap;

/// High-level fleet summary for interactive display.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FleetSummary {
    /// Number of projects in the current assistant scope.
    pub project_count: usize,
    /// Number of projects with cached size values.
    pub measured_count: usize,
    /// Sum of cached `.lake` sizes.
    pub cached_total: u64,
    /// Tag usage counts.
    pub tags: BTreeMap<String, usize>,
}

/// Build a cache summary from indexed project metadata.
pub fn summarize_fleet(projects: &[Project]) -> FleetSummary {
    let mut tags = BTreeMap::new();
    let mut measured_count = 0usize;
    let mut cached_total = 0u64;

    for project in projects {
        if let Some(cache) = &project.size_cache {
            measured_count += 1;
            cached_total += cache.total;
        }
        for tag in &project.tags {
            *tags.entry(tag.clone()).or_insert(0) += 1;
        }
    }

    FleetSummary {
        project_count: projects.len(),
        measured_count,
        cached_total,
        tags,
    }
}
