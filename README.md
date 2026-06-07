# LeanMgr

LeanMgr manages `.lake` as disposable, recoverable cache across many Lean 4
projects. It does not replace Lake, Elan, or Git; it gives users a safe
cross-project view of disk usage, toolchains, worktrees, cache health, and
reclaimable space.

## Why

Lake is intentionally project-local. A user with many Lean projects can end up
with many large `.lake` trees containing dependency checkouts and build products.
These directories are usually reproducible from `lean-toolchain`,
`lakefile.lean` / `lakefile.toml`, and `lake-manifest.json`.

LeanMgr turns the common manual workflow:

```sh
find ~/LeanProjects -name .lake -type d -prune -exec du -sh {} \;
rm -rf old-project/.lake
lake exe cache get
```

into a dry-run-first project management workflow.

## Non-goals

- No Cargo-style global Lean dependency store.
- No automatic `lakefile` rewrites.
- No automatic shared `mathlib` clone.
- No source deletion.
- No Git history changes.
- No Elan state mutation in the MVP.

## Install

One-line install or update (macOS, Linux, Windows via Git Bash/WSL). Prefers
Homebrew when available and falls back to a source build otherwise:

```sh
curl -fsSL https://raw.githubusercontent.com/FrankieeW/leanmgr/main/scripts/install.sh | bash
```

Homebrew directly:

```sh
brew install frankieew/tap/leanmgr
```

From a local checkout, the same script builds from source and installs to
`~/.local/bin` (override with `INSTALL_DIR`):

```sh
scripts/install.sh
INSTALL_DIR=/usr/local/bin scripts/install.sh
```

Script behavior is controlled by environment variables:

| Variable | Effect |
| --- | --- |
| `LEANMGR_NO_BREW=1` | Skip Homebrew even if installed. |
| `LEANMGR_VERSION` | Git tag for the cargo fallback (default `v0.1.0`). |
| `INSTALL_DIR` | Target dir for the local-build path (default `~/.local/bin`). |

## Configuration

LeanMgr stores configuration at:

- Unix/macOS: `$XDG_CONFIG_HOME/leanmgr/config.json`, falling back to
  `~/.config/leanmgr/config.json`
- Windows: `%APPDATA%\leanmgr\config.json`

Initialize it:

```sh
leanmgr init
```

See [examples/config.json](examples/config.json).

## Basic Workflow

Add projects:

```sh
leanmgr add ~/LeanProjects/QuadraticNumberFields --tag msc --tag active
leanmgr scan ~/LeanProjects --yes
```

Inspect:

```sh
leanmgr list
leanmgr list --sizes
leanmgr size --all
leanmgr doctor
leanmgr ai context --format codex
leanmgr toolchain check
leanmgr worktree list
```

Ensure `.lake/` is ignored:

```sh
leanmgr gitignore QuadraticNumberFields --dry-run
leanmgr gitignore --tag active
```

Clean cache safely:

```sh
leanmgr clean QuadraticNumberFields --level soft --dry-run
leanmgr clean --tag archived --level hard --dry-run
leanmgr clean --tag archived --level hard
```

Restore cache with Lake:

```sh
leanmgr restore QuadraticNumberFields
leanmgr restore --tag active
```

## Cleanup Levels

LeanMgr only deletes paths scoped under a selected project's `.lake` directory.

| Level | Removes |
| --- | --- |
| `soft` | `<project>/.lake/build` |
| `deps-build` | `<project>/.lake/packages/*/.lake/build` |
| `hard` | `<project>/.lake` |

All destructive commands support `--dry-run`. Without `--force`, LeanMgr asks
for confirmation before deleting.

## Size Cache

`leanmgr list` is intentionally fast. It reads cached size values from
`config.json` and shows `unknown` / `never` when a project has not been measured
yet.

Refresh cached sizes explicitly:

```sh
leanmgr list --sizes
leanmgr list --tag active --sizes
```

Use `leanmgr size` when you want a dedicated real-time size scan instead of a
quick project index view.

## Commands

```text
leanmgr init
leanmgr add <path> [--name <name>] [--tag <tag>]...
leanmgr remove <name-or-path>
leanmgr list [--tag <tag>] [--sizes] [--json]
leanmgr scan <root> [--yes] [--json]
leanmgr size [<name>] [--tag <tag>] [--all] [--json]
leanmgr doctor [--unused-days 90] [--json]
leanmgr gitignore [<name>] [--tag <tag>] [--all] [--dry-run]
leanmgr clean <name> --level <soft|deps-build|hard> [--dry-run] [--force]
leanmgr clean --tag <tag> --level <soft|deps-build|hard> [--dry-run] [--force]
leanmgr clean --all --level <soft|deps-build|hard> [--dry-run] [--force]
leanmgr restore <name> [--all] [--tag <tag>]
leanmgr tag add <project> <tag>
leanmgr tag remove <project> <tag>
leanmgr tag list
leanmgr ai context [--format <markdown|json|codex|claude>] [--tag <tag>]
leanmgr ai skill list
leanmgr ai skill show leanmgr-cache-manager --format codex
leanmgr ai skill add [source] [--target <auto|codex|claude>] [--dry-run] [--yes]
leanmgr toolchain list
leanmgr toolchain check
leanmgr worktree list
leanmgr worktree doctor
leanmgr worktree prune --dry-run
```

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## Safety Model

The cleanup pipeline is:

```text
Discover targets -> Build cleanup plan -> Print dry-run -> Confirm -> Execute
```

Deletion targets are canonicalized and checked against the selected project's
`.lake` directory before execution. Symlinked targets are refused rather than
followed. LeanMgr does not delete project source directories.

Project selectors (`clean`, `restore`, `gitignore <project>`) fail loudly when
they match no indexed project instead of silently doing nothing.

## Native AI Support

LeanMgr can emit context specifically for coding agents:

```sh
leanmgr ai context --format codex
leanmgr ai context --format claude
leanmgr ai context --format json
```

The context includes project paths, tags, `.lake` sizes, constraints, and a
recommended workflow. It is designed for agents that need a compact project
state before deciding whether to run `doctor`, `clean --dry-run`, `gitignore`,
or `restore`.

LeanMgr knows about one canonical cache-management skill, but the skill body
lives outside the binary:

```sh
leanmgr ai skill list
leanmgr ai skill show leanmgr-cache-manager --format codex
leanmgr ai skill show leanmgr-cache-manager --format claude
```

`skill show` reads from `LEANMGR_SKILL_PATH`, `./SKILL.md`, or
`./leanmgr-cache-manager/SKILL.md`. If none exists, install the skill or point
`LEANMGR_SKILL_PATH` at a local checkout.

To install a skill, LeanMgr first tries the ecosystem command:

```sh
leanmgr ai skill add
```

That runs:

```sh
npx skills add github:FrankieeW/agent-skills/leanmgr-cache-manager
```

The default source points at the future skill repository:
`FrankieeW/agent-skills`. The explicit alias
`leanmgr ai skill add leanmgr-cache-manager` resolves to the same source. If
`npx skills add` is unavailable or fails, LeanMgr prints Codex and Claude Code
fallback skill addresses. Use `--dry-run` to preview the flow, or `--yes` to
skip the fallback prompt.
