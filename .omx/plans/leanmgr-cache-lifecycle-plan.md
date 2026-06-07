# LeanMgr Cache Lifecycle Plan

## Requirements Summary

LeanMgr is a cross-platform Rust CLI for managing many Lean 4 / Lake projects as a
project fleet. It does not replace `lake`, `elan`, or `git`. It provides the
multi-project layer they do not cover: discovery, indexing, disk usage analysis,
safe `.lake` cleanup, restore orchestration, toolchain auditing, worktree auditing,
tagging, and doctor diagnostics.

The core product assumption is:

- `.lake` is disposable, recoverable cache.
- Source files, Git history, and project configuration are not modified by default.
- Official tools remain authoritative:
  - `lake` builds, cleans package build outputs, manages artifact cache, and restores
    mathlib-compatible artifacts.
  - `elan` installs/selects Lean toolchains and manages overrides.
  - `git` manages repositories and worktrees.
- LeanMgr creates plans, reports, and safe orchestration across many projects.

## Product Positioning

LeanMgr should be described as:

> LeanMgr manages `.lake` as disposable, recoverable cache across many Lean 4
> projects. It does not replace Lake or Elan; it gives users a safe cross-project
> view of disk usage, toolchains, worktrees, cache health, and reclaimable space.

## Non-Goals

- Do not implement a Cargo-style global dependency store.
- Do not automatically rewrite `lakefile.toml` or `lakefile.lean`.
- Do not automatically convert `mathlib` to a local path dependency.
- Do not symlink `.lake/packages/mathlib` in MVP.
- Do not delete source directories.
- Do not run `git reset`, rewrite history, or mutate project Git state.
- Do not uninstall elan toolchains in MVP; only report advice.

## Official Tool Boundary

LeanMgr should call or inspect official tools instead of duplicating them:

- `lake clean`: useful for current workspace/package build outputs.
- `lake exe cache get`: restore artifacts where supported.
- `lake cache ...`: Lake cache operations remain Lake's responsibility.
- `elan show`, `elan toolchain list`, `elan override list`: toolchain discovery.
- `git status`, `git branch`, `git worktree list --porcelain`: repository state.

LeanMgr adds value by applying these concepts across a configured set of projects
and by generating safe cleanup plans before any deletion.

## Architecture Decision Record

### Decision

Build LeanMgr as a conservative cache lifecycle manager first. MVP commands should
only read project metadata, report size/status, and delete `.lake`-scoped cache
paths after dry-run and confirmation.

### Drivers

- Users maintain 20-50 Lean projects with repeated `.lake` trees.
- Official Lake/Elan tools are per-project or per-toolchain, not fleet-oriented.
- `.lake` is usually gitignored and recoverable.
- Incorrect deletion can be costly if the tool strays outside `.lake`.
- Shared local mathlib can save space but harms reproducibility unless managed very
  carefully.

### Alternatives Considered

- Global shared mathlib checkout: rejected for MVP because different projects may
  require different Lean and mathlib commits.
- Symlink `.lake/packages/mathlib`: rejected for MVP because it depends on Lake
  internals and can break package assumptions.
- Wrapper around every `lake` command: rejected because it duplicates Lake without
  solving the multi-project pain.
- Direct `rm -rf` helper: rejected because the key product value is safe planning,
  explainability, and recovery workflow.

### Consequences

- MVP is smaller but much safer.
- The strongest initial feature is `doctor` plus cleanup planning.
- Advanced storage optimization remains possible later under explicit experimental
  commands.

## Command Design

### Core MVP

```text
leanmgr init
leanmgr add <path> [--name <name>] [--tag <tag>]...
leanmgr remove <name-or-path>
leanmgr list [--tag <tag>] [--json]
leanmgr scan <root> [--yes] [--json]
leanmgr size [<name>] [--tag <tag>] [--all] [--json]
leanmgr doctor [--unused-days 90] [--json]
leanmgr clean <name> --level <level> [--dry-run] [--force]
leanmgr clean --tag <tag> --level <level> [--dry-run] [--force]
leanmgr clean --all --level <level> [--dry-run] [--force]
leanmgr restore <name> [--all] [--tag <tag>]
```

### Tags

```text
leanmgr tag add <project> <tag>
leanmgr tag remove <project> <tag>
leanmgr tag list
```

### Toolchain Audit

```text
leanmgr toolchain list
leanmgr toolchain check
```

### Worktree Audit

```text
leanmgr worktree list
leanmgr worktree doctor
leanmgr worktree prune --dry-run
leanmgr worktree prune --force
```

MVP `worktree prune` should delegate to `git worktree prune` only. It must not
delete arbitrary source directories.

## Cleanup Levels

Use explicit, `.lake`-scoped levels:

```text
soft:
  Remove <project>/.lake/build

deps-build:
  Remove <project>/.lake/packages/*/.lake/build

hard:
  Remove <project>/.lake
```

Avoid a vague "medium removes all build directories" level. It is too easy to
delete non-Lake source artifacts.

Every destructive command must run through the same pipeline:

```text
Discover targets -> Build cleanup plan -> Print dry-run -> Confirm -> Execute
```

`--dry-run` must never delete. Without `--force`, destructive execution asks:

```text
Delete 12.4 GB?
[y/N]
```

## Configuration

Default location:

- Unix/macOS: `$XDG_CONFIG_HOME/leanmgr/config.json`, falling back to
  `~/.config/leanmgr/config.json`.
- Windows: `%APPDATA%\leanmgr\config.json`.

Initial schema:

```json
{
  "version": 1,
  "projects": [
    {
      "name": "QuadraticNumberFields",
      "path": "~/LeanProjects/QuadraticNumberFields",
      "tags": ["msc", "active"],
      "description": "Imperial MSc project",
      "added_at": "2026-06-07T00:00:00Z",
      "last_seen_at": "2026-06-07T00:00:00Z"
    }
  ]
}
```

Do not persist volatile values such as branch, dirty status, Lean version, Lake
version, or mathlib commit unless a later cache/index file is introduced. Compute
them dynamically in `status`, `doctor`, and `toolchain`.

## Module Structure

```text
src/
  main.rs
  cli.rs
  config.rs
  project.rs
  discovery.rs
  size.rs
  clean.rs
  doctor.rs
  restore.rs
  tags.rs
  toolchain.rs
  worktree.rs
  commands.rs
  output.rs
  paths.rs
  process.rs
  safety.rs
  error.rs
tests/
  fixtures/
  integration_cli.rs
```

Responsibilities:

- `cli`: clap command definitions only.
- `config`: load/save/migrate config.
- `project`: project model and validation.
- `discovery`: recursive scan for `lakefile.toml` / `lakefile.lean`.
- `size`: cross-platform directory size accounting.
- `clean`: cleanup target discovery and execution.
- `doctor`: diagnostics and reclaim recommendations.
- `restore`: calls `lake exe cache get` in project directories.
- `toolchain`: reads `lean-toolchain`, `elan show`, `elan toolchain list`,
  `elan override list`.
- `worktree`: parses `git worktree list --porcelain`.
- `output`: human tables and JSON output.
- `paths`: tilde expansion, config directory, canonicalization.
- `process`: command execution wrapper with captured output.
- `safety`: confirmation prompts, dry-run plan, `.lake` path containment checks.

## Dependency Policy

Required dependencies from the original request:

- `anyhow`
- `clap`
- `colored`
- `indicatif`
- `serde`
- `serde_json`
- `walkdir`

Add only if explicitly accepted during implementation:

- `tempfile` as a dev-dependency for tests.
- `assert_cmd` / `predicates` as dev-dependencies for CLI tests.
- `chrono` or `time` if timestamps need robust parsing/formatting.

If no extra dependencies are allowed, timestamps can be stored as strings and test
fixtures can use temporary directories under the test harness manually.

## Implementation Phases

### Phase 0: Project Scaffold

Create Rust 2024 CLI project with:

- `Cargo.toml`
- `src/main.rs`
- module skeletons
- README
- example config
- install script
- GitHub Actions CI

Acceptance:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`

### Phase 1: Config and Project Index

Implement:

- `init`
- `add`
- `remove`
- `list`
- path expansion
- duplicate detection by canonical path and project name
- Lake project detection via `lakefile.toml` or `lakefile.lean`

Acceptance:

- Adding a valid fixture project writes config.
- Adding a duplicate fails cleanly.
- Removing a project never deletes source.
- `list --json` emits valid JSON.

### Phase 2: Scan and Size

Implement:

- `scan <root>`
- `size`
- table output
- JSON output
- totals

Acceptance:

- Scan finds nested Lake projects and prunes `.lake` traversal.
- Size reports `.lake`, `.lake/build`, `.lake/packages`, and total.
- Missing paths are reported without panic.

### Phase 3: Safe Cleanup

Implement:

- cleanup levels: `soft`, `deps-build`, `hard`
- selection by project, tag, or all
- dry-run plans
- force flag
- confirmation prompt
- `.lake` containment validation

Acceptance:

- `--dry-run` removes nothing.
- Refuses to remove paths outside the selected project's `.lake`.
- Requires confirmation unless `--force`.
- Prints reclaimable total before deletion.

### Phase 4: Doctor

Implement diagnostics:

- largest `.lake`
- largest build directories
- duplicate mathlib checkouts by commit if detectable
- projects unused longer than threshold using `.lake` modification time
- missing project paths
- missing Lake files
- git dirty summary
- toolchain distribution from `lean-toolchain`
- potential reclaim by cleanup level

Acceptance:

- Works when `git`, `lake`, or `elan` are unavailable by degrading gracefully.
- Emits actionable human output and machine-readable JSON.

### Phase 5: Restore

Implement:

- `restore <name>`
- `restore --tag <tag>`
- `restore --all`
- run `lake exe cache get` in each selected project
- progress display and failure summary

Acceptance:

- Uses project working directory.
- Captures exit status per project.
- Continues after one project fails.
- Does not claim success if any restore fails.

### Phase 6: Toolchain and Worktree Views

Implement:

- `toolchain list`
- `toolchain check`
- `worktree list`
- `worktree doctor`
- `worktree prune --dry-run`
- `worktree prune --force`

Acceptance:

- Parses `lean-toolchain` without invoking network.
- Reports installed vs referenced toolchains using `elan toolchain list`.
- Reports override conflicts using `elan override list` when available.
- Uses `git worktree prune`, not manual source deletion.

### Phase 7: Release Hardening

Add:

- README usage examples
- safety documentation
- example config
- install script
- GitHub Actions matrix for macOS, Linux, Windows
- shell completion generation if straightforward through clap

Acceptance:

- CI passes on all three OS families.
- README documents non-goals and deletion guarantees.
- Commands have useful help text.

## Acceptance Criteria

- The tool compiles on Rust 2024 with no warnings.
- All destructive operations support `--dry-run`.
- Destructive operations require confirmation unless `--force`.
- Deletion targets are always proven to be inside `.lake`.
- The tool never modifies Lean source, Lake config, Git history, or elan state in
  MVP.
- Missing external commands produce clear warnings, not panics.
- `doctor` gives a credible reclaim estimate for inactive projects.
- JSON output is available for automation on list/size/doctor/scan.

## Test Plan

### Unit Tests

- config load/save/migration
- path expansion and canonicalization
- project detection
- cleanup target generation
- `.lake` containment checks
- size formatting
- tag add/remove
- parser for `git worktree list --porcelain`
- parser for `elan toolchain list` and `elan override list`

### Integration Tests

- `init -> add -> list -> remove`
- `scan` finds fixture projects
- `size` reports fixture directory totals
- `clean --dry-run` preserves files
- `clean --force` removes only expected `.lake` paths
- `restore` handles fake `lake` command success/failure if process wrapper supports
  test injection

### Manual Validation

- Run on a real Lean project with `.lake`.
- Run on a project without `.lake`.
- Run on a deleted/moved project path.
- Run on a Git worktree.
- Run with no `lake`/`elan` available in PATH if feasible.

## Risks and Mitigations

- Risk: accidental deletion outside `.lake`.
  Mitigation: canonicalize targets and require `.lake` containment before execution.

- Risk: `atime` is unreliable for unused detection.
  Mitigation: use `.lake` modification time and label the result as heuristic.

- Risk: Lake internals change.
  Mitigation: treat only `.lake/build`, `.lake/packages`, and nested package
  `.lake/build` as stable enough for MVP; keep advanced sharing experimental.

- Risk: external command output changes.
  Mitigation: parse only stable/simple formats where available, especially
  `git worktree list --porcelain`.

- Risk: users expect LeanMgr to save disk without rebuild cost.
  Mitigation: README explains the tradeoff: delete cache now, restore/rebuild later.

## Future Experimental Features

Gate these behind explicit `experimental` subcommands or feature flags:

- shared local mathlib checkout advisor
- path dependency migration assistant
- `.lake/packages/mathlib` symlink planner
- content-addressed project cache index
- per-project last-used tracking updated by shell hook
- TUI dashboard

Experimental commands must never silently rewrite project files. They should first
produce a patch or advisory report.

## Suggested Execution Path

For implementation, use a sequential MVP path first:

1. Scaffold and config commands.
2. Add scan/size.
3. Add cleanup safety pipeline.
4. Add doctor.
5. Add restore/toolchain/worktree after the safety core is proven.

Parallelization is useful after Phase 1:

- Lane A: CLI/config/project model.
- Lane B: size/scan fixtures.
- Lane C: safety/cleanup tests.
- Lane D: README/CI/install scripts.

Do not parallelize cleanup execution before the safety model and tests exist.
