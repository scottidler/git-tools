# Implementation Notes: Smooth, in-place `clone --migrate`

Companion to `2026-06-27-clone-migrate-smoothing.md`. Append-only; one section
per phase.

## Phase 1: Spec-optional migrate + main-worktree CWD resolution + absolutize

### Design decisions
- `flat_from_cwd` (`migrate.rs`) resolves the target via `git worktree list
  --porcelain` and takes the **first `worktree <path>` line** as the main
  worktree. Verified against git: from a subdirectory or a linked worktree, the
  first entry is always the main worktree, so this is the one call that handles
  all three cwd shapes. An already-migrated bare container is rejected by
  detecting the `bare` marker line in the same porcelain output.
- `canonicalize` is applied once at the top of `migrate_flat_to_bare`, so every
  derived path (siblings, repair targets) is absolute. This is the root-cause
  fix for rough-edge #1 (relative `--clonepath` breaking `git worktree repair`).

### Deviations
- None.

### Tradeoffs
- `git worktree list` first-entry vs `git rev-parse --show-toplevel`: `--show-toplevel`
  returns the *current* worktree's path (the linked worktree, not the main one),
  which would migrate the wrong directory when run from a legacy linked worktree.
  The worktree-list first entry is the true main worktree, so it was chosen.

### Open questions
- None.

## Phase 2: Preflight (rkvr + ssh key + connectivity) + rescue pass

### Design decisions
- Preflight order (`migrate.rs:migrate_flat_to_bare`): `require_rkvr` -> resolve
  origin -> resolve per-org SSH key -> `git ls-remote` connectivity -> enumerate
  worktrees. All read-only, so any failure leaves the repo byte-for-byte
  unchanged (the panel's finding-1 concern).
- Unmerged/mid-merge detection uses `git diff --name-only --diff-filter=U` per
  worktree and bails BEFORE any mutation (`rescue_work` stage 0).
- The SSH key is threaded into the network ops via a new `envs` param on
  `bare::fix_fetch_refspec` and on `origin_default_branch`.

### Deviations
- **Test conversion timing:** the design doc planned to convert the two
  `refuse`-tests in Phase 4 (accepting a red `test_migrate_refuses_*` between
  Phase 2 and 4). Converted them in Phase 2 instead - the phase that changes the
  behavior - so `otto ci` stays green every phase (the skill's hard rule beats
  the doc's sequencing note). They are now `test_migrate_rescues_dirty_tree` /
  `test_migrate_rescues_nonempty_stash`.
- **`bare::fix_fetch_refspec` scope:** added an `envs` param but the normal clone
  path (`setup_bare_container`, `reconcile_container`) passes `None`, preserving
  its exact prior behavior (ambient SSH for the post-clone refspec fetch). Only
  migrate threads the resolved key. Fixing the latent ambient-SSH gap in the
  normal path was deliberately left out of scope (it is not migrate's job).

### Tradeoffs (Phase 2)
- **Regression test for rough-edge #1 via symlink, not a relative path
  (`test_migrate_canonicalizes_target_path`):** a relative-path test must mutate
  the process-global cwd, which poisons parallel tests' subprocesses (their
  inherited cwd gets deleted when the temp dir drops -> `getcwd() failed`). A
  symlinked input proves the same fix (the returned container is rooted at the
  REAL canonical path, not the symlink) without touching the global cwd.
- `flat_from_cwd` split into a thin wrapper over `flat_from_dir(dir)` so the
  resolution logic is testable without `set_current_dir` (DI over global state).

### Open questions
- None.

## Phase 3: Linked-worktree carry-over + recoverable orphan cleanup

### Design decisions
- `recreate_linked_worktrees` iterates `worktrees.iter().skip(1)` (entry 0 is the
  main worktree) and recreates each linked branch as a native worktree, guarded
  by a `materialized` set (default + current) so a linked worktree on an
  already-checked-out branch is skipped rather than triggering a fatal
  double-checkout. Dir basenames are uniquified via `unique_dir`/`used_dirs`.
- Orphan external dirs are captured up front (`orphan_dirs`) and `rkvr rmrf`'d
  only after a verified swap+repair, and only when still present and not under
  the new container.

### Deviations
- None.

### Tradeoffs
- The injected-post-swap-failure rollback test from the design's Phase 3 test
  list was NOT added: there is no clean seam to force `repair_worktrees`/`verify`
  to fail from a black-box test without a fault-injection hook, and the
  rollback-to-`.backup` path is unchanged from the pre-existing, already-verified
  swap logic. The recoverable-swap behavior remains covered by the original
  design's manual verification; adding a fault-injection port was out of scope.

### Open questions
- None.

## Phase 4: Summary, dropped-state counts, ignored-file + target notes, docs

### Design decisions
- `print_summary` writes to STDERR only (stdout is reserved for the destination
  path per the wrapper contract); it lists rescued `wip/*` branches, carried
  worktrees, removed orphans, ignored files, and the `target` note.
- `ignored_files` uses `git status --porcelain --ignored=traditional` so ignored
  directories collapse (e.g. `target/`) instead of listing every file.
- `warn_dropped_state` now reports counts (custom hooks, HEAD reflog entries).

### Deviations
- Ignored-file detection runs on the MAIN worktree only, not "across all
  worktrees" as the design's preflight sketch said - the high-value case is
  `.env` at the repo root, and per-worktree scanning multiplies summary noise
  for no real gain. Recorded here as a deliberate narrowing.

### Tradeoffs
- The summary is not asserted in tests (it is stderr text); instead the
  Phase-4 tests cover the detection helpers (`ignored_files`, `target_symlink`)
  directly, which is where the logic worth guarding lives.

### Open questions
- Resolved post-Phase-4: (1) persona-location preflight warning - **declined**
  (the user places repos wherever they choose). (2) `--migrate --dry-run` -
  **implemented** (see Phase 5).

## Phase 5: `--migrate --dry-run`

### Design decisions
- `migrate::dry_run` runs the read-only preflight (rkvr/origin/connectivity probe
  + worktree enumeration) and prints the plan to STDERR, then returns the flat
  path so the wrapper leaves the user at the repo (the binary must return a
  path per the wrapper contract; the repo root is the least-surprising choice).
- Default branch for the preview is read via `git ls-remote --symref origin HEAD`
  (`remote_default_branch`) - read-only, no `set-head` mutation.
- `--dry-run` is rejected by `Config` validation unless the op is `Migrate`.
- Extracted `is_dirty` / `has_unmerged` helpers shared by `rescue_work` and
  `dry_run` so the preview and the real run detect the same conditions.

### Deviations
- None.

### Tradeoffs
- The preview reports rkvr-missing / unreachable-origin / unknown-default as
  "real run would abort" rather than aborting itself, so `--dry-run` always
  completes and shows the full plan even when the real run could not proceed.

### Open questions
- None.
