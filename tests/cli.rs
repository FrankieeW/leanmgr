//! End-to-end tests that drive the `leanmgr` binary against a sandboxed
//! config directory. Nothing here touches the real user config or runs
//! `lake`, and Git is isolated from the caller's repository and settings.

use leanmgr::config::Config;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Variables through which a parent Git process (a hook, `git rebase
/// --exec`, a wrapper script) would point child `git` commands at its own
/// repository or configuration.
const INHERITED_GIT_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

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
        fs::write(root.join("gitconfig"), b"").unwrap();
        Self {
            root,
            runs: AtomicUsize::new(0),
        }
    }

    /// A command that sees only the sandbox: its config dir, its own elan
    /// home and Mathlib cache, an empty Git configuration, and no
    /// repository above the sandbox root. A fake `elan` installed with
    /// `fake_elan` comes first on `PATH`.
    fn command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(program);
        for var in INHERITED_GIT_VARS {
            command.env_remove(var);
        }
        command
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("APPDATA", self.root.join("config"))
            .env("ELAN_HOME", self.root.join("elan"))
            .env("MATHLIB_CACHE_DIR", self.root.join("mathlib-cache"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CEILING_DIRECTORIES", &self.root)
            .current_dir(&self.root);
        let bin = self.root.join("bin");
        if bin.is_dir() {
            let inherited = std::env::var_os("PATH").unwrap_or_default();
            let paths = std::iter::once(bin).chain(std::env::split_paths(&inherited));
            command.env("PATH", std::env::join_paths(paths).unwrap());
        }
        command
    }

    /// Put a fake `elan` on the sandbox `PATH`: `toolchain gc --json`
    /// prints `gc_report`, and `toolchain list` lists v4.9.0 as default.
    #[cfg(unix)]
    fn fake_elan(&self, gc_report: &str) {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("gc.json"), gc_report).unwrap();
        let script = "#!/bin/sh\n\
            case \"$*\" in\n\
              \"toolchain gc --json\") cat \"$(dirname \"$0\")/gc.json\" ;;\n\
              \"toolchain list\") echo \"leanprover/lean4:v4.9.0 (default)\" ;;\n\
              *) exit 1 ;;\n\
            esac\n";
        fs::write(bin.join("elan"), script).unwrap();
        fs::set_permissions(bin.join("elan"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Run sandboxed `git` in `dir`.
    fn git(&self, dir: &Path, args: &[&str]) -> std::io::Result<Output> {
        self.command("git")
            .args([
                "-c",
                "user.name=leanmgr",
                "-c",
                "user.email=leanmgr@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
    }

    /// Run `leanmgr` with `stdin`, failing the test if it does not exit
    /// within 30 seconds (guards against interactive loops that never end).
    fn run(&self, args: &[&str], stdin: &str) -> Run {
        self.run_bin(env!("CARGO_BIN_EXE_leanmgr"), args, stdin)
    }

    fn run_bin(&self, binary: &str, args: &[&str], stdin: &str) -> Run {
        let run = self.runs.fetch_add(1, Ordering::SeqCst);
        let out_path = self.root.join(format!("stdout-{run}"));
        let err_path = self.root.join(format!("stderr-{run}"));
        let mut child = self
            .command(binary)
            .args(args)
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
    let sandbox = Sandbox::new("worktree_skips");
    if sandbox.git(&sandbox.root, &["--version"]).is_err() {
        return;
    }
    let plain = sandbox.project("plain", 16, true);
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("plain", &plain), ("gone", &gone)]);

    let run = sandbox.run(&["worktree", "prune", "--dry-run"], "");

    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(run.stdout.contains("Skipped (path missing): gone"));
    assert!(run.stdout.contains("Skipped (not a git repository): plain"));
}

/// Create `repo` with one commit and a linked worktree `wt` whose
/// directory has been deleted, leaving a stale worktree record. Returns
/// `None` when git is unavailable.
fn repo_with_stale_worktree(sandbox: &Sandbox) -> Option<PathBuf> {
    sandbox.git(&sandbox.root, &["init", "-q", "repo"]).ok()?;
    let repo = sandbox.root.join("repo");
    for args in [
        &["commit", "-q", "--allow-empty", "-m", "init"][..],
        &["worktree", "add", "-q", "../wt"][..],
    ] {
        let output = sandbox.git(&repo, args).unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(sandbox.root.join("wt")).unwrap();
    sandbox.write_config(&[("repo", &repo)]);
    Some(repo)
}

#[test]
fn worktree_prune_dry_run_shows_what_git_would_prune() {
    let sandbox = Sandbox::new("worktree_stale");
    if repo_with_stale_worktree(&sandbox).is_none() {
        return;
    }

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
fn worktree_prune_asks_first_and_reports_what_it_pruned() {
    let sandbox = Sandbox::new("worktree_prune_confirm");
    if repo_with_stale_worktree(&sandbox).is_none() {
        return;
    }

    let declined = sandbox.run(&["worktree", "prune"], "n\n");
    assert!(declined.status.success(), "stderr: {}", declined.stderr);
    let prompt_at = declined.stdout.find("[y/N]").expect("confirmation prompt");
    let preview_at = declined.stdout.find("worktrees/wt").expect("preview line");
    assert!(preview_at < prompt_at, "preview must precede the prompt");
    assert!(declined.stdout.contains("Aborted."));
    let still_broken = sandbox.run(&["worktree", "doctor"], "");
    assert!(still_broken.stdout.contains("Broken worktrees: 1"));

    let forced = sandbox.run(&["worktree", "prune", "--force"], "");
    assert!(forced.status.success(), "stderr: {}", forced.stderr);
    assert!(
        forced.stdout.contains("worktrees/wt"),
        "the real prune reports what it removed:\n{}",
        forced.stdout
    );
    let fixed = sandbox.run(&["worktree", "doctor"], "");
    assert!(fixed.stdout.contains("Broken worktrees: 0"));
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
fn lm_generates_completions_for_lm() {
    let sandbox = Sandbox::new("completions_lm");
    let run = sandbox.run_bin(env!("CARGO_BIN_EXE_lm"), &["completions", "bash"], "");
    assert!(run.status.success(), "stderr: {}", run.stderr);
    assert!(
        run.stdout.contains("_lm()"),
        "bash function for lm:\n{}",
        run.stdout
    );
}

#[test]
fn gc_json_never_prompts_and_reports_whether_it_executed() {
    let sandbox = Sandbox::new("gc_json");
    let project = sandbox.project("old", 1024, true);
    sandbox.write_config(&[("old", &project)]);

    let planned = sandbox.run(&["gc", "--unused-days", "0", "--json"], "");
    assert!(planned.status.success(), "stderr: {}", planned.stderr);
    let report: serde_json::Value =
        serde_json::from_str(&planned.stdout).expect("stdout is pure JSON (no prompt)");
    assert_eq!(report["executed"], false);
    assert_eq!(report["mode"]["unused_days"], 0);
    assert_eq!(report["targets"].as_array().unwrap().len(), 1);
    assert!(project.join(".lake").exists());

    let forced = sandbox.run(&["gc", "--unused-days", "0", "--json", "--force"], "");
    let report: serde_json::Value = serde_json::from_str(&forced.stdout).unwrap();
    assert_eq!(report["executed"], true);
    assert!(!project.join(".lake").exists());
}

#[test]
fn restoring_a_named_project_whose_path_is_gone_fails() {
    let sandbox = Sandbox::new("restore_missing_named");
    let gone = sandbox.missing("gone");
    sandbox.write_config(&[("gone", &gone)]);

    let run = sandbox.run(&["restore", "gone"], "");

    assert!(!run.status.success());
    assert!(run.stderr.contains("cannot restore gone"));
}

#[test]
fn scan_rejects_a_missing_root() {
    let sandbox = Sandbox::new("scan_missing_root");
    let run = sandbox.run(&["scan", "does-not-exist"], "");
    assert!(!run.status.success());
    assert!(run.stderr.contains("scan root is not a directory"));
}

#[cfg(unix)]
#[test]
fn doctor_reports_shared_caches_and_unused_toolchains() {
    let sandbox = Sandbox::new("doctor_shared");
    let project = sandbox.project("demo", 16, true);
    sandbox.write_config(&[("demo", &project)]);
    fs::create_dir_all(sandbox.root.join("mathlib-cache")).unwrap();
    fs::write(sandbox.root.join("mathlib-cache/a.ltar"), vec![0u8; 100]).unwrap();
    let toolchain = sandbox
        .root
        .join("elan/toolchains/leanprover--lean4---v4.8.0/bin");
    fs::create_dir_all(&toolchain).unwrap();
    fs::write(toolchain.join("lean"), vec![0u8; 50]).unwrap();
    sandbox
        .fake_elan(r#"{"unused_toolchains": ["leanprover/lean4:v4.8.0"], "used_toolchains": []}"#);

    let json = sandbox.run(&["doctor", "--json"], "");
    assert!(json.status.success(), "stderr: {}", json.stderr);
    let report: serde_json::Value = serde_json::from_str(&json.stdout).unwrap();
    assert!(
        report["potential_hard_reclaim"].is_u64(),
        "existing keys stay"
    );
    let shared = &report["shared_caches"];
    assert_eq!(shared["mathlib_downloads"]["bytes"], 100);
    assert_eq!(shared["elan_toolchains"]["bytes"], 50);
    assert_eq!(
        shared["unused_toolchains"][0]["toolchain"],
        "leanprover/lean4:v4.8.0"
    );
    assert_eq!(shared["unused_toolchains"][0]["bytes"], 50);

    let human = sandbox.run(&["doctor"], "");
    assert!(
        human
            .stdout
            .contains("`elan toolchain gc --delete` removes 1 toolchain(s)"),
        "{}",
        human.stdout
    );

    let check = sandbox.run(&["toolchain", "check"], "");
    assert!(check.stdout.contains("Unused installed toolchains"));
    assert!(check.stdout.contains("leanprover/lean4:v4.8.0"));
    assert!(check.stdout.contains("installed"), "v4.9.0 is installed");
}

#[cfg(unix)]
#[test]
fn scan_from_elan_adds_known_lake_projects_only() {
    let sandbox = Sandbox::new("scan_from_elan");
    let known = sandbox.project("known", 16, true);
    let dependency = known.join(".lake/packages/dep");
    fs::create_dir_all(&dependency).unwrap();
    fs::write(dependency.join("lakefile.toml"), b"name = \"dep\"").unwrap();
    let report = serde_json::json!({
        "unused_toolchains": [],
        "used_toolchains": [
            {"user": known.display().to_string(), "toolchain": "leanprover/lean4:v4.9.0"},
            {"user": dependency.display().to_string(), "toolchain": "leanprover/lean4:v4.9.0"},
            {"user": "default toolchain", "toolchain": "leanprover/lean4:v4.9.0"}
        ]
    });
    sandbox.fake_elan(&report.to_string());

    let preview = sandbox.run(&["scan", "--from-elan"], "");
    assert!(preview.status.success(), "stderr: {}", preview.stderr);
    assert!(preview.stdout.contains("known  "));
    assert!(!preview.stdout.contains("packages/dep"));

    let added = sandbox.run(&["scan", "--from-elan", "--yes"], "");
    assert!(
        added.stdout.contains("Added 1 project(s)"),
        "{}",
        added.stdout
    );
    let again = sandbox.run(&["scan", "--from-elan"], "");
    assert!(again.stdout.contains("(already indexed)"));
}

#[test]
fn gc_budget_and_age_filter_combine() {
    let sandbox = Sandbox::new("gc_combined");
    let project = sandbox.project("old", 1024, true);
    sandbox.write_config(&[("old", &project)]);

    let run = sandbox.run(
        &["gc", "--unused-days", "0", "--target", "1KiB", "--json"],
        "",
    );

    assert!(run.status.success(), "stderr: {}", run.stderr);
    let report: serde_json::Value = serde_json::from_str(&run.stdout).unwrap();
    assert_eq!(report["mode"]["unused_days"], 0);
    assert_eq!(report["mode"]["target_bytes"], 1024);
    assert_eq!(report["targets"].as_array().unwrap().len(), 1);

    let neither = sandbox.run(&["gc"], "");
    assert!(!neither.status.success());
}
