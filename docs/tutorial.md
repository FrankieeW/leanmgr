# leanmgr Tutorial

A first-time walkthrough: from a fresh install to reclaiming disk space with
`leanmgr gc`. About five minutes.

## 1. Install and initialize

```sh
brew install frankieew/tap/leanmgr
leanmgr init
```

`leanmgr init` creates the config file (empty project list). Add `leanmgr` to
your shell's search path if needed.

## 2. Add a project

```sh
leanmgr add ~/LeanProjects/MyProject
leanmgr list
```

`leanmgr add` validates that the path contains `lakefile.toml` or
`lakefile.lean`, then writes a record into the config. Repeat for each
project, or use `leanmgr scan <root> --yes` to discover them in bulk.

## 3. Inspect disk usage

```sh
leanmgr list --sizes        # refresh and show cached sizes
leanmgr size --all          # live size scan across all projects
leanmgr doctor              # health summary
leanmgr doctor --json       # machine-readable for pipelines
```

`leanmgr list` is fast (reads the size cache). `leanmgr size` walks the
filesystem — slower but always fresh. `doctor` surfaces projects whose
path is missing, whose Lake file is gone, or whose `.lake` mtime is older
than `--unused-days`.

## 4. Plan cleanup (always dry-run first)

```sh
leanmgr gc --unused-days 90 --dry-run
leanmgr gc --target 20GiB  --dry-run
leanmgr gc --tag archived  --dry-run
```

`--unused-days N` selects projects whose `.lake` mtime is older than
`N` days. `--target SIZE` greedily picks the largest recoverable caches
until the budget is met. `--tag` narrows the scope. Add `--json` for
machine-readable output:

```sh
leanmgr gc --unused-days 90 --dry-run --json
# {
#   "mode": { "unused_days": 90 },
#   "targets": [...],
#   "skipped": [...],
#   "total_bytes": 1234567890,
#   "executed": false
# }
```

## 5. Execute

```sh
leanmgr gc --unused-days 90                # prompt, then delete
leanmgr gc --unused-days 90 --force        # no prompt
leanmgr gc --target 20GiB --force          # free ≥ 20 GiB
leanmgr gc --unused-days 0 --include-unrecoverable --force
```

`--force` skips the confirmation prompt. `gc` defaults to `--level hard`
(removes the entire `.lake`); use `--level soft` or `--level deps-build`
for narrower scopes.

## 6. Restore when you come back to a project

```sh
leanmgr restore MyProject
leanmgr restore --tag archived --all
```

`leanmgr restore` shells out to `lake exe cache get`, which re-fetches
the Lake dependencies recorded in `lake-manifest.json`. This is what
makes gc safe: nothing is destroyed that `lake` cannot rebuild.

## 7. JSON scripts (CI / automation)

```sh
# Plan-only, parse the report, do not delete:
plan=$(leanmgr gc --target 20GiB --dry-run --json)

# Or delete, and assert via the executed field:
result=$(leanmgr gc --target 20GiB --force --json)
executed=$(echo "$result" | jq .executed)
[ "$executed" = "true" ] || { echo "gc did not execute"; exit 1; }
```

## Safety model

| Flag | Effect |
| --- | --- |
| (default) | gc only acts on **recoverable** caches — projects with `lake-manifest.json`, a Lake file (`lakefile.toml` or `lakefile.lean`), and a non-empty `lean-toolchain`. Unrecoverable caches are listed in the `skipped` section. |
| `--include-unrecoverable` | Include unrecoverable caches. Use this only if you understand the risk. |
| `--dry-run` | Print the plan, do not delete. |
| `--force` | Skip the confirm prompt. |
| `--json` | Emit a structured report. In JSON mode, gc never prompts interactively — `--force` is the only way to execute. The report's `executed: bool` field tells you what happened. |

## Where to go next

- `leanmgr --help` — command list
- `leanmgr <command> --help` — flag details
- `leanmgr ai context --format codex` — agent-friendly project snapshot
