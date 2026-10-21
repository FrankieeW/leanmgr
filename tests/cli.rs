//! End-to-end tests that drive the `leanmgr` binary against a sandboxed
//! config directory. Nothing here touches the real user config or runs
//! `lake`.

use leanmgr::config::Config;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A temporary directory holding a private config dir plus project trees.
struct Sandbox {
    root: PathBuf,
    runs: AtomicUsize,
}

struct Run {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("leanmgr-it-{name}-{nonce}"));
        fs::create_dir_all(root.join("config")).unwrap();
        Self {
            root,
            runs: AtomicUsize::new(0),
        }
    }

    /// Run `leanmgr` with `stdin`, failing the test if it does not exit
    /// within 30 seconds (guards against interactive loops that never end).
    fn run(&self, args: &[&str], stdin: &str) -> Run {
        let run = self.runs.fetch_add(1, Ordering::SeqCst);
        let out_path = self.root.join(format!("stdout-{run}"));
        let err_path = self.root.join(format!("stderr-{run}"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_leanmgr"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("APPDATA", self.root.join("config"))
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(File::create(&out_path).unwrap())
            .stderr(File::create(&err_path).unwrap())
            .spawn()
            .unwrap();
        if let Some(mut input) = child.stdin.take() {
            // The child may exit before reading everything; that is fine.
            let _ = input.write_all(stdin.as_bytes());
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("leanmgr {args:?} did not exit within 30s");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        Run {
            status,
            stdout: fs::read_to_string(out_path).unwrap(),
            stderr: fs::read_to_string(err_path).unwrap(),
        }
    }

    /// Create a Lake project with a `.lake` of `lake_bytes` bytes. A
    /// recoverable project also gets a manifest and a toolchain file.
    fn project(&self, name: &str, lake_bytes: usize, recoverable: bool) -> PathBuf {
        let dir = self.root.join(name);
        fs::create_dir_all(dir.join(".lake")).unwrap();
        fs::write(dir.join(".lake/blob"), vec![b'x'; lake_bytes]).unwrap();
        fs::write(dir.join("lakefile.toml"), format!("name = \"{name}\"\n")).unwrap();
        if recoverable {
            fs::write(
                dir.join("lake-manifest.json"),
                br#"{"version": "1.1.0", "packages": []}"#,
            )
            .unwrap();
            fs::write(dir.join("lean-toolchain"), b"leanprover/lean4:v4.9.0\n").unwrap();
        }
        dir
    }

    fn write_config(&self, projects: &[(&str, &Path)]) {
        let projects: Vec<serde_json::Value> = projects
            .iter()
            .map(|(name, path)| serde_json::json!({"name": name, "path": path.display().to_string()}))
            .collect();
        let config = serde_json::json!({"version": 1, "projects": projects});
        let dir = self.root.join("config").join("leanmgr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }

    fn read_config(&self) -> Config {
        let path = self.root.join("config").join("leanmgr").join("config.json");
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn missing(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn gc_shows_the_plan_before_asking_and_keeps_cache_on_no() {
    let sandbox = Sandbox::new("gc_prompt_order");
    let project = sandbox.project("old", 1024, true);
    sandbox.write_config(&[("old", &project)]);

    let run = sandbox.run(&["gc", "--unused-days", "0"], "n\n");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    let plan_at = run.stdout.find("WOULD REMOVE").expect("plan table");
    let prompt_at = run.stdout.find("Delete ").expect("confirmation prompt");
    assert!(
        plan_at < prompt_at,
        "plan must precede prompt:\n{}",
        run.stdout
    );
    assert!(run.stdout.contains("Aborted."));
    assert!(project.join(".lake").exists());
}

#[test]
fn gc_force_removes_recoverable_cache_and_skips_unrecoverable() {
    let sandbox = Sandbox::new("gc_force");
    let ok = sandbox.project("ok", 1024, true);
    let bare = sandbox.project("bare", 1024, false);
    sandbox.write_config(&[("ok", &ok), ("bare", &bare)]);

    let run = sandbox.run(&["gc", "--unused-days", "0", "--force"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(!ok.join(".lake").exists());
    assert!(bare.join(".lake").exists());
    assert!(run.stdout.contains("Skipped (unrecoverable"));
}

#[test]
fn clean_with_nothing_to_remove_says_so() {
    let sandbox = Sandbox::new("clean_empty");
    let project = sandbox.project("demo", 16, true);
    sandbox.write_config(&[("demo", &project)]);

    let run = sandbox.run(&["clean", "demo", "--level", "soft", "--dry-run"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert_eq!(run.stdout.trim(), "Nothing to clean.");
}

#[test]
fn size_with_unknown_project_fails_loudly() {
    let sandbox = Sandbox::new("size_unknown");
    let project = sandbox.project("demo", 16, true);
    sandbox.write_config(&[("demo", &project)]);

    let run = sandbox.run(&["size", "nope"], "");

    assert!(!run.status.success());
    assert!(run.stderr.contains("project not found: nope"));
}

#[test]
fn remove_missing_previews_confirms_and_removes_only_missing_entries() {
    let sandbox = Sandbox::new("remove_missing");
    let kept = sandbox.project("kept", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("kept", &kept), ("gone", &gone)]);

    let preview = sandbox.run(&["remove", "--missing", "--dry-run"], "");
    assert!(preview.status.success(), "stderr: {}", preview.stderr);
    assert!(preview.stdout.contains("gone"));
    assert!(!preview.stdout.contains("kept"));
    assert_eq!(sandbox.read_config().projects.len(), 2);

    let declined = sandbox.run(&["remove", "--missing"], "n\n");
    assert!(declined.stdout.contains("Aborted."));
    assert_eq!(sandbox.read_config().projects.len(), 2);

    let forced = sandbox.run(&["remove", "--missing", "--force"], "");
    assert!(forced.status.success(), "stderr: {}", forced.stderr);
    let names: Vec<String> = sandbox
        .read_config()
        .projects
        .into_iter()
        .map(|project| project.name)
        .collect();
    assert_eq!(names, vec!["kept".to_string()]);
    assert!(kept.exists(), "source directories are never touched");
}

#[test]
fn list_flags_missing_paths() {
    let sandbox = Sandbox::new("list_missing");
    let kept = sandbox.project("kept", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("kept", &kept), ("gone", &gone)]);

    let run = sandbox.run(&["list"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    let gone_row = run
        .stdout
        .lines()
        .find(|line| line.starts_with("gone"))
        .expect("row for the missing project");
    assert!(gone_row.contains("missing"));
    assert!(run.stdout.contains("leanmgr remove --missing"));
}

#[test]
fn interact_pipe_mode_ends_at_end_of_input() {
    let sandbox = Sandbox::new("interact_eof");
    let project = sandbox.project("demo", 16, true);
    sandbox.write_config(&[("demo", &project)]);

    let run = sandbox.run(&["interact"], "1\n");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.stdout.trim_end().ends_with("Done."));
    assert!(!run.stdout.contains("Unknown action"));
}

#[test]
fn interact_reports_a_bad_answer_and_keeps_going() {
    let sandbox = Sandbox::new("interact_bad_answer");
    let project = sandbox.project("demo", 16, true);
    sandbox.write_config(&[("demo", &project)]);

    let run = sandbox.run(&["interact"], "4\n99\n\nq\n");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.stdout.contains("Error: project index out of range: 99"));
    assert!(run.stdout.trim_end().ends_with("Done."));
}

#[test]
fn restore_dry_run_explains_what_runs_and_what_is_skipped() {
    let sandbox = Sandbox::new("restore_plan");
    let with_mathlib = sandbox.project("withmathlib", 16, true);
    fs::write(
        with_mathlib.join("lake-manifest.json"),
        br#"{"version": "1.1.0", "packages": [{"type": "git", "name": "mathlib"}]}"#,
    )
    .unwrap();
    let plain = sandbox.project("plain", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[
        ("withmathlib", &with_mathlib),
        ("plain", &plain),
        ("gone", &gone),
    ]);

    let run = sandbox.run(&["restore", "--all", "--dry-run"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    let row = |name: &str| {
        run.stdout
            .lines()
            .find(|line| line.starts_with(name))
            .unwrap_or_else(|| panic!("no row for {name}:\n{}", run.stdout))
            .to_string()
    };
    assert!(row("withmathlib").contains("lake exe cache get"));
    assert!(row("plain").contains("skip: no Mathlib dependency"));
    assert!(row("gone").contains("skip: path missing"));
}

#[test]
fn toolchain_list_does_not_count_missing_paths_as_toolchains() {
    let sandbox = Sandbox::new("toolchain_missing");
    let pinned = sandbox.project("pinned", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("pinned", &pinned), ("gone", &gone)]);

    let run = sandbox.run(&["toolchain", "list"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.stdout.contains("leanprover/lean4:v4.9.0"));
    assert!(!run.stdout.contains("missing  "));
    assert!(
        run.stdout
            .contains("Skipped 1 project(s) whose path no longer exists")
    );
}

#[test]
fn worktree_prune_dry_run_skips_projects_git_cannot_audit() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let sandbox = Sandbox::new("worktree_skips");
    let plain = sandbox.project("plain", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("plain", &plain), ("gone", &gone)]);

    let run = sandbox.run(&["worktree", "prune", "--dry-run"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.stdout.contains("Skipped (path missing): gone"));
    assert!(run.stdout.contains("Skipped (not a git repository): plain"));
}

#[test]
fn worktree_prune_dry_run_shows_what_git_would_prune() {
    let git = |dir: &Path, args: &[&str]| {
        Command::new("git")
            .args([
                "-c",
                "user.name=leanmgr",
                "-c",
                "user.email=leanmgr@example.com",
            ])
            .args(args)
            .current_dir(dir)
            .output()
    };
    let sandbox = Sandbox::new("worktree_stale");
    if git(&sandbox.root, &["init", "-q", "repo"]).is_err() {
        return;
    }
    let repo = sandbox.root.join("repo");
    assert!(
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])
            .unwrap()
            .status
            .success()
    );
    assert!(
        git(&repo, &["worktree", "add", "-q", "../wt"])
            .unwrap()
            .status
            .success()
    );
    fs::remove_dir_all(sandbox.root.join("wt")).unwrap();
    sandbox.write_config(&[("repo", &repo)]);

    let preview = sandbox.run(&["worktree", "prune", "--dry-run"], "");
    assert!(preview.status.success(), "stderr: {}", preview.stderr);
    assert!(
        preview.stdout.contains("worktrees/wt"),
        "dry run must show the stale worktree:\n{}",
        preview.stdout
    );

    let doctor = sandbox.run(&["worktree", "doctor"], "");
    assert!(doctor.stdout.contains("Broken worktrees: 1"));
}

#[test]
fn completions_are_generated_for_every_shell() {
    let sandbox = Sandbox::new("completions");
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let run = sandbox.run(&["completions", shell], "");
        assert!(run.status.success(), "{shell}: {}", run.stderr);
        assert!(
            run.stdout.contains("leanmgr"),
            "{shell} script names the binary"
        );
    }
}

#[test]
fn scan_rejects_a_missing_root() {
    let sandbox = Sandbox::new("scan_missing_root");
    let run = sandbox.run(&["scan", "does-not-exist"], "");
    assert!(!run.status.success());
    assert!(run.stderr.contains("scan root is not a directory"));
}
