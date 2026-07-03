# Implementation Notes: Flat-by-default clone + worktree fleet hygiene

Companion to `2026-07-03-clone-flat-default.md`. Append-only, one section per
phase. This plan spans two repos (git-tools: phases 0a/1/2/3; gx: phase 0b), so
notes are maintained centrally here by the execution orchestrator from each
phase-implementer's report.

<!-- phase sections appended below as phases complete -->

## Phase 0a: Unify worktree enumeration in `common`

Repo: git-tools. Commit: `d5c88b4`. CI: green.

### Design decisions
- `WorktreeRow` fields match the doc exactly (path, branch, head, bare, locked) — `common/src/bare.rs`.
- Kept `clone/src/migrate.rs`'s own `Worktree{head: String}` type as-is; `list_worktrees` just calls the shared `resolve_worktrees` parser and maps `Option<String>` head to `String`. Collapsing migrate's helpers into `common` is explicitly Phase 2 scope.

### Deviations
- None from the Phase 0a spec.

### Tradeoffs
- `worktree/src/list.rs`: used `impl From<WorktreeRow> for Entry` rather than inlining the field mapping, keeping `Entry` distinct from `WorktreeRow` (no `head` field) since prune/pick only reference path/branch/bare/locked.

### Open questions
- None.

## Phase 0b: Teach `gx` both layouts (vendored module)

Repo: gx. Commit: `18c9b55`. CI: green (10 bare unit tests + 2 integration tests).

### Design decisions
- **Container definition (resolves the doc's open question):** `bare::is_bare_container` (`src/bare.rs`) is STRICT — requires ALL of: `.bare/` is a directory, `.git` is a regular file, and its contents start with `gitdir:` and reference `.bare`. A lone `.bare/` dir, a `.git` directory (flat repo), or a bare worktree is NOT a container. `.bare`-is-dir checked first for fast-fail on the ~99% non-container dirs.
- Container = ONE logical repo — `Repo::from_container` + `discover_repos` (`src/repo.rs`): emits a single Repo named for the container dir with path = default worktree, so git runs in a real work tree and writes never fan out N×.
- Walk pruning — `is_inside_bare_container` (`src/repo.rs`) in `filter_entry`: prunes a container's `.bare/` and worktree children from re-discovery.
- Layout-aware origin — `extract_origin_url` (`src/repo.rs`): fast direct `.git/config` read for flat repos, falls back to `bare::origin_url` for worktrees/containers.
- Recorded-path validation via `bare::is_git_path` in `validate_recovery_state` (`src/rollback.rs`) and the cleanup path resolver (`src/cleanup.rs`).

### Deviations
- Module is ~205 lines vs the doc's "~50 line" estimate; the extra is doc comments, a `Worktree` struct, and per-function debug logging (logging rule). Logic itself is minimal. No functional deviation.

### Tradeoffs
- Vendored `src/bare.rs` vs depending on `common` — chose vendoring per Alternative 2 (both reviewers rejected the cross-repo dep).
- `is_bare_container` called per-directory in discovery vs caching — chose the simple per-dir call; the `.bare`-is-dir short-circuit keeps it to one stat in the common case.

### Open questions
- None blocking. `--bare`-alias question is Phase 1's concern.

### Deviations from spec
- Doc line numbers didn't map 1:1 to current gx; the actual assumptions were fixed instead. Notably `find_workspace_root`'s `current.join(".git").exists()` (doc's `:151`) already tolerates a `.git` pointer file, so it needed no change.

## Phase 1: Flip `clone` default to flat, add `--bare`

Repo: git-tools. Commit: `c5cbd33`. CI: green (clone: 51 unit + 9 integration).

### Design decisions
- Precedence in `resolve_layout(bare_flag, flat_flag, versioning, cfg_layout)` (`clone/src/config.rs:135`): flat-forcing CLI flags first (defensive) > `[clone] default-layout` cfg (only `"bare"` case-insensitively selects Bare; unset/garbage falls through to Flat) > built-in default `Flat`. NO env layer.
- Conflicting CLI flags rejected up front in `Config::try_from` (`clone/src/config.rs:79-89`): `--bare` with `--flat`/`--versioning`, and `--bare` with `--migrate`. Keeps `resolve_layout` a pure, easily-tested function.
- `--bare` added to `clone/src/cli.rs:38`; `--flat` retained as no-op alias; `--versioning` still implies flat; `after_help` documents the new default + escape hatches.

### Deviations
- Inverted `test_clone_existing_public_repo_succeeds` → `test_clone_default_produces_flat_layout` (integration) since it pinned the old bare default and would have gone red; added `test_clone_bare_opt_in_layout` to preserve bare-container coverage under the new opt-in. Same intent as the doc, which only named the unit test explicitly.

### Tradeoffs
- Hard-error validation of `--bare` conflicts (matching the existing `--flat`/`--versioning`+`--migrate` pattern) vs silent precedence in `resolve_layout`.

### Open questions
- **ACTION FOR USER:** the `clone` skill doc at `~/repos/scottidler/claude/HOME/.claude/skills/clone/SKILL.md` (separate repo `scottidler/claude`) still documents bare-by-default and `--flat` as the opt-out — now backwards. Needs a separate commit/PR in that repo. Not edited here (out of scope for these two repos).

## Phase 2: Safe bare->flat collapse (`clone --flatten`)

Repo: git-tools. Commit: `2fa080a`. CI: green (`otto ci` exit 0; 20 flatten unit tests pass).

### Orchestration note (execution incident)
- The first delegated `phase-2` agent died mid-refactor (unreachable), leaving the helper-lift half-done (orphaned local defs in `migrate.rs` causing dead-code clippy failures). A retry `phase-2b` was spawned with a precise handoff of the in-tree state. A transient interrupted the session mid-run, but `phase-2b` recovered and completed the phase: wrote `flatten.rs` + `flatten/tests.rs`, got otto ci green, committed `2fa080a`. The orchestrator independently re-ran `otto ci` (exit 0) and the flatten suite (20/20 pass) and reviewed both files against the safety contract before accepting. No Phase 2 code was hand-written by the orchestrator; its own drafts were correctly rejected because the agent's real files were already on disk.

### Design decisions
- Helper-lift into `common` (`common/src/rkvr.rs` `require`/`rmrf`; `common/src/bare.rs` `is_dirty`/`assert_all_clean`); `migrate.rs` rewired to them and its old local defs removed (the dead-code fix).
- Refuse-first `inspect()` (`clone/src/flatten.rs`) collects ALL refusal reasons (so `--dry-run` shows the full list; the real run bails on non-empty). Checks: existing `refs/stash`; per-worktree dirty/untracked (`common::bare::is_dirty`); in-progress merge/rebase/cherry-pick/revert/bisect via private-gitdir markers (`in_progress_operation`); per-worktree config + sparse-checkout (`per_worktree_state`); dirty/conflicted submodules (`dirty_submodules`); branch-not-ancestor-of-default (`is_ancestor`, ancestry not `diff-filter=U`); detached HEAD unreachable from any ref (`commit_reachable_from_ref`).
- Copy-based crash-safe transition (`perform_transition`): recursive `copy_dir_all` of live `.bare/`->`<repo>.flattening/.git/` (never mutates the live store; replicates symlinks without following them); `core.bare=false` + unset `core.worktree` + `core.logAllRefUpdates=true` via `git config --file`; `remote.origin.fetch` LEFT UNTOUCHED (never introduces the clobbering `+refs/heads/*:refs/heads/*`); drop staged `worktrees/` admin entries; pin `HEAD` to default then `reset --hard`; verify (status clean + identical `refs/*` OIDs + `cat-file -e` each OID + `fsck --connectivity-only`) BEFORE the atomic rename swap; re-verify at the final path; `rkvr rmrf` the backup only after final verification; rollback restores the backup on any post-swap failure.
- `container_from_cwd` resolves via `git rev-parse --git-common-dir` so it works from the container root, a worktree, or a subdir; rejects a flat checkout.

### Deviations
- None from the contract. The `is_bare_container` used is the existing clone-local (loose `.bare/`) detector; strict detection was assigned to Phase 0b/gx, so git-tools' detector is unchanged (in scope).

### Tradeoffs
- A pre-swap failure (e.g. `reset --hard` on a bad default) leaves the `<repo>.flattening` staging dir behind (cleared by the next run's `rkvr::rmrf`), mirroring migrate. The live container is always intact (proven by `test_flatten_transition_failure_leaves_live_bare_intact`). Not worth a pre-swap cleanup path.
- `copy_dir_all` is a hand-rolled recursive copy (portable, symlink-faithful) rather than `cp -a`.

### Open questions
- None.

## Phase 3: E2E lifecycle tests

Repo: git-tools. Commit: `a8414dc`. CI: green (`otto ci` exit 0; new suite 3/3 pass).

### Design decisions
- New file `clone/tests/lifecycle_tests.rs` drives the BUILT BINARY (the seam `integration_tests.rs` uses), covering the multi-step structural lifecycle the module-level tests do not.
- Hermetic fixture: a local bare "remote" at `<root>/remotes/<org>/<repo>` passed via `--remote`, so `clone`'s URL-join resolves to a local path and `ls-remote`/`fetch`/`clone` run offline; `--migrate`'s connectivity probe and `--flatten`'s rkvr-gated transition both run with no network.
- Every transition addressed by an explicit repospec + `--clonepath` (never cwd-relative), keeping the e2e independent of `flat_from_cwd`/`container_from_cwd` (already unit-covered).
- A `MARKER.txt` const asserted byte-for-byte after every transition proves content survives the flat->bare->flat cycle, not just directory shape.
- Tests gated on `rkvr_available()` (mirrors `src/flatten/tests.rs`); rkvr v0.1.22 present, so all 3 ran.

### Deviations
- None from Phase 3 scope. The doc also lists "gx treating a container as one repo" under Phase 3; that was already delivered in Phase 0b (gx's own `tests/bare_layout_test.rs`), so no gx change was needed here.

### Tradeoffs
- New `lifecycle_tests.rs` vs appending to `integration_tests.rs` (kept single-invocation clone tests separate from multi-step lifecycle tests).
- Used `--bare` (not migrate-from-flat) to build the container for the round-trip/dry-run tests; forward migration is proven in its own test rather than chained back-to-back.

### Open questions
- None.

---

## Cross-repo summary

Two repos, five phases, all `otto ci` green and independently re-verified by the orchestrator:

| Phase | Repo | Commit |
|-------|------|--------|
| 0a unify `resolve_worktrees` | git-tools | `d5c88b4` |
| 0b gx layout-aware | gx | `18c9b55` |
| 1 flat default + `--bare` | git-tools | `c5cbd33` |
| 2 `--flatten` collapse | git-tools | `2fa080a` |
| 3 e2e lifecycle | git-tools | `a8414dc` |

**Follow-up owned by the user:** update the `clone` skill doc at `~/repos/scottidler/claude/HOME/.claude/skills/clone/SKILL.md` (separate `scottidler/claude` repo) for the new flat-by-default behavior; it still documents bare-by-default.
