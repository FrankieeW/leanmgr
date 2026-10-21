//! Caches shared by every Lean project on the machine: Mathlib's artifact
//! download cache and elan's installed toolchains.
//!
//! LeanMgr only measures them and points at the owning tool's cleanup
//! command; it never deletes them itself.

use crate::paths::home_dir;
use crate::process::stdout;
use crate::size::dir_size;
use serde::Serialize;
use serde_json::Value;
use std::env;
use std::path::{Path, PathBuf};

/// Arguments for elan's report-only toolchain garbage collection. Never
/// add `--delete` here: removing toolchains is elan's decision to make
/// when the user runs it.
const ELAN_GC_REPORT: [&str; 3] = ["toolchain", "gc", "--json"];

/// One shared cache directory and its size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CacheDir {
    /// Directory path.
    pub path: PathBuf,
    /// Bytes stored under the directory.
    pub bytes: u64,
}

/// A toolchain that `elan toolchain gc` reports as unused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UnusedToolchain {
    /// Toolchain name, e.g. `leanprover/lean4:v4.9.0`.
    pub toolchain: String,
    /// Installed size, when its directory could be found.
    pub bytes: Option<u64>,
}

/// Machine-wide Lean caches that live outside every project.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SharedCaches {
    /// Mathlib's download cache, filled by `lake exe cache get`.
    pub mathlib_downloads: Option<CacheDir>,
    /// elan's installed toolchains.
    pub elan_toolchains: Option<CacheDir>,
    /// Toolchains no known project uses, per `elan toolchain gc`. `None`
    /// when elan, or its experimental `toolchain gc` command, is unavailable.
    pub unused_toolchains: Option<Vec<UnusedToolchain>>,
}

impl SharedCaches {
    /// Total size of the unused toolchains whose size is known.
    pub fn unused_toolchain_bytes(&self) -> u64 {
        self.unused_toolchains
            .iter()
            .flatten()
            .filter_map(|toolchain| toolchain.bytes)
            .sum()
    }
}

/// Measure the shared caches.
pub fn collect_shared_caches() -> SharedCaches {
    SharedCaches {
        mathlib_downloads: mathlib_cache_dir().and_then(|dir| measure(&dir)),
        elan_toolchains: elan_home().and_then(|home| measure(&home.join("toolchains"))),
        unused_toolchains: unused_toolchains(),
    }
}

/// Toolchains no known project uses, with their installed sizes, per
/// `elan toolchain gc`. `None` when that report is unavailable.
pub fn unused_toolchains() -> Option<Vec<UnusedToolchain>> {
    let names = parse_unused_toolchains(&elan_gc_report()?)?;
    let elan = elan_home();
    Some(
        names
            .into_iter()
            .map(|toolchain| UnusedToolchain {
                bytes: elan
                    .as_ref()
                    .and_then(|home| measure(&toolchain_dir(home, &toolchain)))
                    .map(|dir| dir.bytes),
                toolchain,
            })
            .collect(),
    )
}

/// Projects elan has seen Lean used in, from `elan toolchain gc --json`.
///
/// elan lists every known project directory that still exists, alongside
/// entries such as the default toolchain that are not paths; only absolute
/// paths are returned. `None` when the report is unavailable.
pub fn elan_known_project_dirs() -> Option<Vec<PathBuf>> {
    let report = elan_gc_report()?;
    let used = report.get("used_toolchains")?.as_array()?;
    Some(
        used.iter()
            .filter_map(|entry| entry.get("user").and_then(Value::as_str))
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .collect(),
    )
}

fn measure(path: &Path) -> Option<CacheDir> {
    path.is_dir().then(|| CacheDir {
        path: path.to_path_buf(),
        bytes: dir_size(path).unwrap_or(0),
    })
}

/// Mathlib's cache directory, resolved the way Mathlib's `cache` tool does:
/// `MATHLIB_CACHE_DIR`, then `$XDG_CACHE_HOME/mathlib`, then
/// `~/.cache/mathlib`.
fn mathlib_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("MATHLIB_CACHE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(cache) = env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(cache).join("mathlib"));
    }
    home_dir()
        .ok()
        .map(|home| home.join(".cache").join("mathlib"))
}

/// elan's home directory: `ELAN_HOME`, else `~/.elan`.
fn elan_home() -> Option<PathBuf> {
    env::var_os("ELAN_HOME")
        .map(PathBuf::from)
        .or_else(|| home_dir().ok().map(|home| home.join(".elan")))
}

/// Directory elan installs `toolchain` into: `/` becomes `--` and `:`
/// becomes `---`, so `leanprover/lean4:v4.9.0` lives in
/// `toolchains/leanprover--lean4---v4.9.0`.
fn toolchain_dir(elan_home: &Path, toolchain: &str) -> PathBuf {
    elan_home
        .join("toolchains")
        .join(toolchain.replace('/', "--").replace(':', "---"))
}

fn elan_gc_report() -> Option<Value> {
    let output = stdout("elan", &ELAN_GC_REPORT).ok().flatten()?;
    serde_json::from_str(&output).ok()
}

/// Read `unused_toolchains` from an `elan toolchain gc --json` report.
/// Entries are toolchain names; objects carrying a `toolchain` field are
/// accepted too.
fn parse_unused_toolchains(report: &Value) -> Option<Vec<String>> {
    let entries = report.get("unused_toolchains")?.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|entry| match entry {
                Value::String(name) => Some(name.clone()),
                Value::Object(fields) => fields
                    .get("toolchain")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                _ => None,
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elan_gc_is_only_ever_asked_for_a_report() {
        assert!(!ELAN_GC_REPORT.contains(&"--delete"));
        assert!(ELAN_GC_REPORT.contains(&"--json"));
    }

    #[test]
    fn toolchain_dir_matches_elan_naming() {
        assert_eq!(
            toolchain_dir(Path::new("/elan"), "leanprover/lean4:v4.36.0-rc1"),
            Path::new("/elan")
                .join("toolchains")
                .join("leanprover--lean4---v4.36.0-rc1")
        );
    }

    #[test]
    fn parses_unused_toolchains_as_names_or_objects() {
        let report: Value = serde_json::from_str(
            r#"{"unused_toolchains": ["leanprover/lean4:v4.8.0",
                                      {"toolchain": "leanprover/lean4:v4.9.0"}, 3],
                "used_toolchains": []}"#,
        )
        .unwrap();
        assert_eq!(
            parse_unused_toolchains(&report),
            Some(vec![
                "leanprover/lean4:v4.8.0".to_string(),
                "leanprover/lean4:v4.9.0".to_string(),
            ])
        );
        assert_eq!(
            parse_unused_toolchains(&serde_json::json!({"other": 1})),
            None
        );
    }

    #[test]
    fn unused_toolchain_bytes_sums_known_sizes() {
        let caches = SharedCaches {
            unused_toolchains: Some(vec![
                UnusedToolchain {
                    toolchain: "a".to_string(),
                    bytes: Some(10),
                },
                UnusedToolchain {
                    toolchain: "b".to_string(),
                    bytes: None,
                },
                UnusedToolchain {
                    toolchain: "c".to_string(),
                    bytes: Some(5),
                },
            ]),
            ..SharedCaches::default()
        };
        assert_eq!(caches.unused_toolchain_bytes(), 15);
        assert_eq!(SharedCaches::default().unused_toolchain_bytes(), 0);
    }
}
