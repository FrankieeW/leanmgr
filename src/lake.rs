//! Read-only helpers for the Lake files in a project root.
//!
//! Every command that asks "is this a Lake project?", "which toolchain
//! does it pin?", or "does it depend on Mathlib?" goes through here so
//! the answers stay consistent across `scan`, `doctor`, `gc`, `restore`,
//! and `toolchain`.

use serde_json::Value;
use std::fs;
use std::path::Path;

/// Lake configuration file names.
pub const LAKEFILES: [&str; 2] = ["lakefile.toml", "lakefile.lean"];

/// Return true when `root` contains `lakefile.toml` or `lakefile.lean`.
pub fn has_lakefile(root: &Path) -> bool {
    LAKEFILES.iter().any(|name| root.join(name).exists())
}

/// Return the toolchain pinned by `root/lean-toolchain`: its first
/// non-empty line, trimmed. `None` when the file is missing or blank.
pub fn read_toolchain(root: &Path) -> Option<String> {
    let content = fs::read_to_string(root.join("lean-toolchain")).ok()?;
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

/// Return true when the project at `root` is Mathlib or depends on it,
/// i.e. when `lake exe cache get` has prebuilt artifacts to download.
///
/// Either source of truth is enough. `lake-manifest.json` lists every
/// resolved package, including transitive ones; the Lake file catches a
/// `require mathlib` added since the manifest was last written (or a
/// project with no manifest yet). Erring towards "yes" is deliberate:
/// a wrong "yes" costs one failed `cache get`, a wrong "no" sends the user
/// to build Mathlib from source.
pub fn requires_mathlib(root: &Path) -> bool {
    let manifest = fs::read_to_string(root.join("lake-manifest.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok());
    manifest.as_ref().is_some_and(manifest_mentions_mathlib)
        || LAKEFILES.iter().any(|name| {
            fs::read_to_string(root.join(name))
                .is_ok_and(|content| lakefile_mentions_mathlib(&content))
        })
}

/// Recognize the ways a Lake file names Mathlib as a dependency (or is
/// Mathlib): TOML `name = "mathlib"`, Lean `require mathlib ...` or
/// `require "leanprover-community" / "mathlib"`, and Mathlib's own
/// `package mathlib`. A bare substring would also match unrelated names
/// such as `mathlibStandardSet` or a package called `withmathlib`.
fn lakefile_mentions_mathlib(content: &str) -> bool {
    content.contains("\"mathlib\"")
        || content.contains("require mathlib")
        || content.contains("package mathlib")
}

fn manifest_mentions_mathlib(manifest: &Value) -> bool {
    if manifest.get("name").and_then(Value::as_str) == Some("mathlib") {
        return true;
    }
    manifest
        .get("packages")
        .and_then(Value::as_array)
        .is_some_and(|packages| packages.iter().any(package_is_mathlib))
}

fn package_is_mathlib(package: &Value) -> bool {
    // Current manifests store the name on the entry itself; pre-v7
    // manifests nest it under the source kind (`{"git": {"name": ...}}`).
    let name = package
        .get("name")
        .or_else(|| package.get("git").and_then(|git| git.get("name")));
    name.and_then(Value::as_str) == Some("mathlib")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("leanmgr-lake-{name}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn has_lakefile_accepts_either_spelling() {
        let root = tmp("has_lakefile");
        assert!(!has_lakefile(&root));
        fs::write(root.join("lakefile.lean"), b"import Lake").unwrap();
        assert!(has_lakefile(&root));
        fs::remove_file(root.join("lakefile.lean")).unwrap();
        fs::write(root.join("lakefile.toml"), b"name = \"demo\"").unwrap();
        assert!(has_lakefile(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn read_toolchain_skips_blank_lines_and_trims() {
        let root = tmp("toolchain");
        assert_eq!(read_toolchain(&root), None);
        fs::write(root.join("lean-toolchain"), b"  \n").unwrap();
        assert_eq!(read_toolchain(&root), None);
        fs::write(
            root.join("lean-toolchain"),
            b"\n leanprover/lean4:v4.9.0 \n",
        )
        .unwrap();
        assert_eq!(
            read_toolchain(&root).as_deref(),
            Some("leanprover/lean4:v4.9.0")
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manifest_with_mathlib_package_requires_mathlib() {
        let root = tmp("manifest_dep");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": "1.1.0", "name": "Demo",
                 "packages": [{"type": "git", "name": "batteries"},
                              {"type": "git", "name": "mathlib"}]}"#,
        )
        .unwrap();
        assert!(requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn mathlib_itself_requires_mathlib() {
        let root = tmp("manifest_self");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": "1.3.0", "name": "mathlib",
                 "packages": [{"type": "git", "name": "batteries"}]}"#,
        )
        .unwrap();
        assert!(requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn legacy_nested_manifest_entries_are_recognized() {
        let root = tmp("manifest_legacy");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": 5, "packages": [{"git": {"name": "mathlib", "rev": "abc"}}]}"#,
        )
        .unwrap();
        assert!(requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stale_manifest_does_not_hide_a_new_mathlib_require() {
        // `require mathlib` was added after the manifest was last written;
        // skipping restore here would send the user to build Mathlib.
        let root = tmp("manifest_stale");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": "1.1.0", "name": "Demo", "packages": [{"type": "git", "name": "batteries"}]}"#,
        )
        .unwrap();
        fs::write(
            root.join("lakefile.lean"),
            b"import Lake\nrequire mathlib from git \"https://github.com/leanprover-community/mathlib4\"\n",
        )
        .unwrap();
        assert!(requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn incidental_mentions_of_mathlib_do_not_count() {
        let root = tmp("lakefile_incidental");
        fs::write(
            root.join("lake-manifest.json"),
            br#"{"version": "1.1.0", "name": "withmathlib", "packages": []}"#,
        )
        .unwrap();
        fs::write(
            root.join("lakefile.toml"),
            b"name = \"withmathlib\"\n# not using mathlib yet\nweak.linter.mathlibStandardSet = true\n",
        )
        .unwrap();
        assert!(!requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lean_lakefile_forms_are_recognized() {
        assert!(lakefile_mentions_mathlib(
            "require \"leanprover-community\" / \"mathlib\" @ git \"v4.9.0\""
        ));
        assert!(lakefile_mentions_mathlib(
            "require mathlib from \"../mathlib4\""
        ));
        assert!(lakefile_mentions_mathlib(
            "package mathlib where\n  leanOptions := #[]"
        ));
        assert!(!lakefile_mentions_mathlib("package Demo where"));
    }

    #[test]
    fn lakefile_requirement_is_used_without_manifest() {
        let root = tmp("lakefile_dep");
        fs::write(
            root.join("lakefile.toml"),
            b"name = \"Demo\"\n\n[[require]]\nname = \"mathlib\"\n",
        )
        .unwrap();
        assert!(requires_mathlib(&root));
        fs::write(root.join("lakefile.toml"), b"name = \"Demo\"\n").unwrap();
        assert!(!requires_mathlib(&root));
        fs::remove_dir_all(&root).unwrap();
    }
}
