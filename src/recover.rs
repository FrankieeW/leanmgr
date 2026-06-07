//! Offline recoverability assessment for project `.lake` caches.

use crate::project::Project;
use std::fs;

/// Whether a project's `.lake` can be rebuilt after deletion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recoverability {
    /// The project has the inputs needed to restore its cache.
    Recoverable,
    /// The cache cannot be cheaply rebuilt; the string explains why.
    Unrecoverable(String),
}

/// Assess recoverability using only local files. No network, no process spawn.
///
/// Recoverable iff `lake-manifest.json` exists, a Lake file
/// (`lakefile.toml` or `lakefile.lean`) is present, and `lean-toolchain`
/// exists and is non-empty.
pub fn assess(project: &Project) -> Recoverability {
    let root = project.expanded_path();
    if !root.join("lake-manifest.json").exists() {
        return Recoverability::Unrecoverable("no lake-manifest.json".to_string());
    }
    if !root.join("lakefile.toml").exists() && !root.join("lakefile.lean").exists() {
        return Recoverability::Unrecoverable("no lakefile.toml or lakefile.lean".to_string());
    }
    match fs::read_to_string(root.join("lean-toolchain")) {
        Ok(content) if !content.trim().is_empty() => Recoverability::Recoverable,
        _ => Recoverability::Unrecoverable("no lean-toolchain".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Project;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn tmp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("leanmgr-{name}-{nonce}"))
    }

    fn project_at(root: &Path) -> Project {
        Project {
            name: "demo".to_string(),
            path: root.display().to_string(),
            tags: Vec::new(),
            description: None,
            added_at: None,
            last_seen_at: None,
            last_committed_at: None,
            size_cache: None,
        }
    }

    #[test]
    fn recoverable_with_manifest_and_toolchain() {
        let root = tmp("recover_ok");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("lakefile.toml"), b"").unwrap();
        fs::write(root.join("lake-manifest.json"), b"{}").unwrap();
        fs::write(root.join("lean-toolchain"), b"leanprover/lean4:v4.0.0\n").unwrap();
        assert_eq!(assess(&project_at(&root)), Recoverability::Recoverable);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unrecoverable_without_lakefile() {
        let root = tmp("recover_no_lakefile");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("lake-manifest.json"), b"{}").unwrap();
        fs::write(root.join("lean-toolchain"), b"leanprover/lean4:v4.0.0\n").unwrap();
        assert_eq!(
            assess(&project_at(&root)),
            Recoverability::Unrecoverable("no lakefile.toml or lakefile.lean".to_string())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unrecoverable_without_manifest() {
        let root = tmp("recover_no_manifest");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("lean-toolchain"), b"leanprover/lean4:v4.0.0\n").unwrap();
        assert_eq!(
            assess(&project_at(&root)),
            Recoverability::Unrecoverable("no lake-manifest.json".to_string())
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn unrecoverable_with_empty_toolchain() {
        let root = tmp("recover_empty_toolchain");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("lakefile.toml"), b"").unwrap();
        fs::write(root.join("lake-manifest.json"), b"{}").unwrap();
        fs::write(root.join("lean-toolchain"), b"   \n").unwrap();
        assert_eq!(
            assess(&project_at(&root)),
            Recoverability::Unrecoverable("no lean-toolchain".to_string())
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
