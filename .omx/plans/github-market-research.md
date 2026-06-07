# GitHub Market Research for LeanMgr

## Search Summary

Searches run through GitHub repository search and web search did not find a
direct equivalent to LeanMgr: a local, cross-project Lean 4 `.lake` cache
lifecycle manager.

The closest projects are adjacent infrastructure:

- official Lean toolchain/version management
- official or community GitHub Actions for Lean CI
- Bazel integration for Lean/Lake
- Lean 4 project templates
- older Lean 3 `leanproject` workflow

## Direct Competitor Check

No direct competitor found for:

- multi-project `.lake` discovery
- local disk usage ranking across Lean projects
- safe batch deletion of inactive `.lake` directories
- cross-project `lake exe cache get` restore orchestration
- doctor-style diagnosis of duplicate mathlib checkouts, stale worktrees, and
  reclaimable disk space

## Relevant Projects

### leanprover/elan

URL: https://github.com/leanprover/elan

What it does:

- Manages Lean toolchains.
- Selects Lean/Lake based on `lean-toolchain` or directory override.
- Installs, lists, uninstalls, and garbage-collects toolchains.

Relevance:

- LeanMgr should not duplicate toolchain installation.
- LeanMgr can add a multi-project audit view over `lean-toolchain`,
  `elan toolchain list`, and `elan override list`.

Borrow:

- Rust CLI quality bar.
- Clear boundary around what a version manager owns.

### leanprover/lean-action

URL: https://github.com/leanprover/lean-action

What it does:

- Standard GitHub Action for Lean project CI.
- Builds, tests, lints, and optionally runs extra checks.
- Caches `.lake` in GitHub Actions.
- Distinguishes GitHub `.lake` caching from Mathlib cache hydration via
  `lake exe cache get`.

Relevance:

- Confirms `.lake` is treated as cache in normal Lean workflow.
- Its cache key design is a useful reference:
  - OS
  - architecture
  - Lake manifest
  - git commit hash

Borrow:

- Auto-detect when a Lake workspace supports a feature.
- Let explicit user flags override auto-configuration.
- Report per-feature status rather than a single opaque success/failure.

### coproduct-opensource/lean-mathlib-cache

URL: https://github.com/coproduct-opensource/lean-mathlib-cache

What it does:

- Reusable GitHub Action for Lean CI cache hygiene.
- Installs elan, resolves `lean-toolchain`, caches `~/.elan`, `.lake`, and
  Mathlib-related cache paths.
- Runs `lake exe cache get`.
- Uses cache keys based on `lean-toolchain`, `lakefile.lean`/`lakefile.toml`,
  and `lake-manifest.json`.

Relevance:

- Very close philosophically, but CI-only.
- It packages the repeated Lean cache workflow for GitHub Actions; LeanMgr can
  package the repeated local cache workflow for many projects.

Borrow:

- Cache identity should be based on:
  - `lean-toolchain`
  - `lakefile.lean`
  - `lakefile.toml`
  - `lake-manifest.json`
- Warn against sharing one cache key across multiple Lean projects.
- Treat `lake exe cache get` as optional because not every project depends on
  Mathlib.

### fastverk/rules_lean

URL: https://github.com/fastverk/rules_lean

What it does:

- Bazel rules for Lean 4 with Lake integration.
- Reads `lean-toolchain`, `lakefile`, and `lake-manifest.json`.
- Runs `lake update` and `lake exe cache get` for mathlib dependencies.
- Exposes Lake packages as Bazel targets.
- Provides structured introspection of `.olean` files and Lake workspaces.

Relevance:

- Not a Lean project cache manager, but it validates the same metadata boundary:
  `lean-toolchain` + `lakefile` + `lake-manifest.json`.
- Shows a more aggressive hermetic-build direction that LeanMgr should not pursue
  in MVP.

Borrow:

- Treat `lake-manifest.json` as the lockfile for resolved dependency commits.
- Separate "what is pinned" from "what still reaches the network".
- Keep internals explicitly unstable if later adding `.olean` or workspace
  introspection.

### leanprover-community/LeanProject

URL: https://github.com/leanprover-community/LeanProject

What it does:

- Template for Lean 4 formalization projects.
- Includes standard `lean-toolchain`, `lakefile.toml`, `lake-manifest.json`,
  VS Code settings, CI, and project structure.

Relevance:

- Useful fixture/reference for what a typical Lean 4 math project looks like.
- Confirms LeanMgr should detect both repo roots and nested Lean project roots.

Borrow:

- README wording around expected Lean project files.
- Test fixtures can mimic this structure.

### Lean 3 leanproject

URL: https://leanprover-community.github.io/leanproject.html

What it did:

- Older Lean 3-era project workflow tool.
- `leanproject get` fetched existing projects and prepared mathlib.
- `leanproject get-cache` was the predecessor workflow now replaced by
  `lake exe cache get` in Lean 4.

Relevance:

- Historical precedent for a helper CLI around Lean project ergonomics.
- It was more project setup oriented than local cache lifecycle oriented.

Borrow:

- Simple workflow commands that hide repetitive Lean setup steps.
- Avoid copying its scope directly because Lean 4 moved build/dependency
  responsibility into Lake.

## Product Gap

The gap is local and operational:

- CI tools cache `.lake` for one repository.
- `lake` manages one workspace at a time.
- `elan` manages toolchains, not projects.
- Bazel integrations manage hermetic builds, not normal Lean user workspaces.
- Templates create projects, but do not maintain 20-50 existing projects.

LeanMgr should own the missing layer:

```text
Find all Lean projects -> explain disk usage -> recommend cache deletion ->
delete only `.lake`-scoped targets -> restore with official Lake commands.
```

## Design Implications

- MVP should stay local-first and offline-friendly.
- Do not claim global dependency deduplication.
- Make `doctor` the differentiating feature.
- Model cache identity around `lean-toolchain`, `lakefile`, and
  `lake-manifest.json`.
- Add `restore` as orchestration around `lake exe cache get`, not a custom cache
  implementation.
- Add `toolchain check` as read-only aggregation over `elan`.
- Add CI docs later, but do not make CI the core product.

## Competitive Positioning

Short version:

> Existing tools help one Lean project build faster. LeanMgr helps one user keep
> dozens of Lean projects from quietly consuming disk.

Long version:

> LeanMgr complements Lake, Elan, and Lean CI actions by managing disposable
> `.lake` state across many local projects. It gives users a safe dry-run-first
> workflow for reclaiming disk, finding stale caches, restoring Mathlib artifacts,
> and diagnosing toolchain/worktree drift.
