# Design Document: Smooth, in-place `clone --migrate`

**Author:** Scott Idler
**Date:** 2026-06-27
**Status:** Implemented
**Review Passes Completed:** 5/5 + cross-model panel (Architect/Gemini, Staff Engineer/Codex) incorporated

## Summary

Make `clone --migrate` something you run from *inside* the checkout you want to
convert, with no `org/repo` argument, that safely handles every rough edge found
during the `tatari-tv/marquee` migration instead of leaving a punch list for the
operator. It derives the target from the current directory, auto-rescues all
git-tracked uncommitted work and stashes into `wip/*` branches, carries linked
worktrees into the new container, and cleans up the orphans recoverably. Every
removal goes through `rkvr rmrf`; the existing rename-aside swap remains the
rollback backstop; and — critically — the rescue pass is **additive** (it only
adds refs and moves dirty work to stashes), so no failure can rewrite or delete
committed history.

## Problem Statement

### Background

`clone` produces a bare-container + nested-worktree layout (design:
`clone/docs/design/2026-06-21-clone-bare-worktree.md`). `--migrate` converts a
legacy flat checkout into that layout. The mechanism works — it builds the bare
container from the LOCAL repo (preserving unpushed commits and local-only
branches), stages it as `<repo>.migrating`, verifies, then does a recoverable
rename swap (`<repo>` -> `<repo>.backup` -> `<repo>.migrating` -> `<repo>`),
re-verifies, and only then removes the backup.

A real migration of `tatari-tv/marquee` (handoff: `clone/docs/2026-06-26-migrate-rough-edges.md`)
showed it works but is rough: it required a workaround to run at all, and left
six things for the operator to fix by hand.

### Problem

The current `--migrate` (`clone/src/migrate.rs`, `clone/src/lib.rs:34-42`):

1. **Requires a repospec and a correct `--clonepath`.** `flat` is built as
   `config.clonepath.join(spec)` (`lib.rs:39`). With the default relative
   `clonepath = "."`, the post-swap `git worktree repair` re-resolves relative
   paths against the wrong cwd and fails (`fatal: Invalid path '.../<org>'`).
2. **`ensure_clean` checks only the main tree** (`migrate.rs:121-141`). Dirty
   *linked* worktrees are invisible; their changes would be lost silently.
3. **Linked worktrees are orphaned**, not carried over.
4. **Stashes cause a hard refuse** with no preservation path.
5. **The `target` build-dir symlink** (relocate-targets) is dropped silently.
6. **Dropped machine-local state** (reflogs, custom hooks) is warned about
   vaguely.

### Goals

- Run `clone --migrate` with **no arguments** from inside the flat checkout (or
  any subdirectory, or even a legacy linked worktree of it) and do the right
  thing.
- **Lose nothing git-tracked.** Every form of *tracked or stashed* work —
  uncommitted changes, untracked (non-ignored) files, stashes, unpushed commits,
  local-only branches, detached-HEAD worktree commits — survives as a reachable
  git ref. Git-ignored files are detected, listed, and remain recoverable via
  `rkvr rcvr` (see Non-Goals).
- Carry **linked worktrees** into the new container automatically.
- Keep all removals **recoverable** (`rkvr rmrf`) and keep the rollback
  guarantee intact, including across the new rescue pass.
- Leave the operator with a clear **summary**, not a punch list.

### Non-Goals

- **Preserving git-ignored files automatically.** `git stash --include-untracked`
  does not capture `.gitignore`d files (`.env`, local configs, build dirs).
  Migrate **detects and lists** them in the summary and relies on the
  `rkvr rmrf`'d `<repo>.backup` being recoverable via `rkvr rcvr`; it does not
  copy them into the new worktrees (the common ignored payload is large build
  output we explicitly do not want to duplicate). The "Lose nothing" claim is
  therefore scoped to git-tracked/stashed state. *(Panel finding 2; user
  decision: warn + recoverable.)*
- **Recreating the `target` symlink** (rough-edge #5). relocate-targets owns
  build-dir relocation; migrate prints a pointer and stops.
- **Auto-initializing submodules.** `.gitmodules` and gitlinks are preserved
  (verified: content is not lost); the operator runs `git submodule update
  --init` as after any clone.
- **A separate permanent backup.** `rkvr rmrf` (recoverable, harvested), never
  `rkvr bkup` (never harvested).
- **Changing the bare-worktree layout, the persona invariant, or the wrapper
  contract.**

## Proposed Solution

### Overview

`--migrate` becomes spec-optional. With no spec it resolves the flat checkout's
**main worktree** from the enclosing repo (so it works from a subdirectory *or*
a legacy linked worktree), absolutized via `canonicalize` — which also fixes
rough-edge #1 (no relative path survives to confuse `git worktree repair`).

A **preflight** (entirely read-only) runs first: require `rkvr`, resolve the
per-org SSH key from the `origin` URL, verify remote connectivity, and enumerate
the worktree set. Only after preflight passes does the **rescue pass** run —
additive by construction (it creates `wip/*` refs and moves dirty work into
stashes; it never rewrites or deletes a commit or branch). Because the bare
container is cloned from the LOCAL repo, those `refs/heads/wip/*` are captured
automatically; `refs/stash` is not (verified), which is why the conversion to
branches is necessary.

After the swap, previously-linked worktrees are recreated natively inside the
container and their orphaned external directories are `rkvr rmrf`'d.

### Control flow (`migrate_flat_to_bare`)

```
0. RESOLVE: from CWD, find the enclosing repo's MAIN worktree
     (git worktree list --porcelain, first entry; handles subdir + legacy
      linked-worktree cwd). Reject if already a bare container. canonicalize.
1. PREFLIGHT (read-only; failure => repo byte-for-byte unchanged):
     a. require `rkvr` on PATH (bail if absent — no non-recoverable deletes)
     b. resolve per-org SSH key from the origin URL
     c. connectivity precheck: git ls-remote origin (with that key); bail on failure
     d. enumerate worktrees (main + linked), capturing path, branch, HEAD sha
     e. detect git-ignored files across all worktrees (for the summary)
2. RESCUE PASS (additive; only ADDS wip/* refs + moves dirty work to stashes):
     a. detached-HEAD worktrees -> git branch wip/detached-<shortsha> <sha>
     b. each dirty worktree     -> git stash push --include-untracked
     c. every stash entry       -> git branch wip/<slug|stash-i> stash@{i}
            (prefix-safe + length-capped naming)
3. assert EVERY worktree clean (loop the full set, not just main)
4. capture origin URL, current branch, target-symlink target; summarize dropped state
5. git clone --bare <flat> -> <repo>.migrating/.bare  (captures refs/heads/* incl wip/*)
6. set-url origin <real>; write .git pointer; fix fetch refspec (with resolved key); fetch
7. detect TRUE default branch from remote; add default worktree; reset HEAD;
   add the previously-checked-out branch's worktree (if != default)
8. recreate each linked worktree whose branch is NOT already materialized
9. verify staged default worktree
10. swap: rename flat -> .backup, .migrating -> flat   (rollback on failure)
11. git worktree repair <all new worktree absolute paths>; re-verify
        (rollback to .backup on failure)
12. rkvr rmrf the orphaned external linked dirs (skip any under flat)
13. rkvr rmrf .backup
14. print summary to STDERR; return <flat>/<default>
```

### Failure model and invariants *(Panel finding 1 — the convergent must-fix)*

The rescue pass mutates the source repo *before* the rename-swap, so the
rename-swap rollback (`migrate.rs:91-108`) is not the whole story. The design's
guarantee is stated in terms of an **invariant**, not a single rollback point:

> **Once the rescue pass begins, no committed history or branch is ever rewritten
> or deleted.** Rescue only (a) creates `wip/*` refs and (b) moves dirty working
> changes into stashes (themselves recoverable). The worst-case failure outcome
> is a *source repo decorated with extra `wip/*` branches and a clean working
> tree*, plus a removable `<repo>.migrating` — never lost tracked work.

Failure behavior by stage:
- **Stage 0–1 (resolve + preflight):** read-only. Any failure leaves the repo
  byte-for-byte unchanged. The `rkvr` check, SSH-key resolution, and `ls-remote`
  connectivity probe all happen here, so the network/credential failure that
  would otherwise strike at step 6 (after rescue) is caught *before* any mutation.
- **Stage 2 (rescue):** additive. A failure mid-rescue leaves prior `wip/*` refs
  and stashes in place (work preserved) and bails with a message listing what was
  created. `git stash push` on an unmerged/mid-merge tree is fatal — preflight
  does **not** auto-resolve it; it is reported and migration stops *before* any
  `wip/*` is created for that tree (the cleanest bail point).
- **Stage 5–9 (stage the container):** builds `<repo>.migrating` alongside; the
  source is only *decorated* with `wip/*`. A failure bails, removes
  `<repo>.migrating`, and leaves the source intact (original branches + `wip/*`).
- **Stage 10–11 (swap + repair):** the existing recoverable rename + rollback to
  `<repo>.backup`.

Re-running `--migrate` after a partial failure is safe: clean trees re-stash to
nothing, and `wip/*` naming is collision-safe, so a re-run adds at most a few
duplicate rescue branches — never data loss.

### Data Model

```rust
/// One entry from `git worktree list --porcelain`.
struct Worktree {
    path: PathBuf,          // absolute working-dir path
    branch: Option<String>, // None when detached
    head: String,           // the HEAD sha (needed for wip/detached-<sha>)  <-- panel finding 4
    is_main: bool,          // the flat checkout itself
}
```

Sets threaded through the build:
- `materialized: HashSet<String>` — branch names already turned into a worktree
  (default, and current-if-different). Guards against `git worktree add` on a
  branch already checked out, which is **fatal** (verified).
- `used_dirs: HashSet<String>` — worktree directory basenames, to uniquify slug
  collisions (`feature/x` and `feature-x` both slug to `feature-x`).
- The `wip/*` naming guard is **prefix-aware**, not flat-equality *(panel finding
  5)*: in `refs/heads/`, `wip/foo` and `wip/foo/bar` are mutually exclusive
  (file-vs-directory), and a bare `wip` branch blocks any `wip/*`. Our generated
  names are always single-segment (`slugify_branch` collapses `/` to `-`), so our
  names cannot conflict with *each other*; but the candidate must be checked
  against existing refs for path-prefix conflicts (candidate is a prefix of, or
  has as a prefix, any existing `refs/heads/*`). On a detected conflict, suffix
  and retry; as a backstop, a `git branch` failure falls through to the next
  candidate rather than aborting the run.

### API Design

```rust
// lib.rs
Op::Migrate => {
    let flat = match &config.spec {
        Some(spec) => config.clonepath.join(spec.to_string()),
        None => migrate::flat_from_cwd()?,
    };
    migrate::migrate_flat_to_bare(&flat, config.default_branch.as_deref())
}

// migrate.rs
pub fn flat_from_cwd() -> Result<PathBuf>;   // resolve enclosing repo's MAIN worktree
pub fn migrate_flat_to_bare(flat: &Path, default_fallback: Option<&str>) -> Result<PathBuf>;
```

`flat_from_cwd` resolves the **main worktree** of the enclosing repo (the first
entry of `git worktree list --porcelain`), so it works from a subdirectory and
from a *legacy* flat repo's linked worktree alike *(panel finding 9)*. It rejects
an already-migrated layout: if the enclosing repo is a bare container
(`is_bare_container`), there is nothing to migrate.

All operator-facing summary output goes to **STDERR**. Per the wrapper contract
(`git-tools/CLAUDE.md`), the binary prints **only** the destination path to
stdout. Renaming the user's own CWD mid-run is safe because every path is
absolute post-canonicalize and the wrapper `cd`s to the returned path after exit.

### SSH credentials for the no-spec path *(panel finding 3)*

With a spec, `Config` resolves the per-org SSH key (`config.rs:93`); with no
spec there is no spec to key off. Migrate therefore resolves the key itself: it
parses the `origin` URL into an org (via `common::git::parse_repospec` /
`parse_git_url`), calls `config::find_ssh_key_for_org`, and applies the key as a
`GIT_SSH_COMMAND` env override (the third arg of `git::run`/`git::output`, using
`git::ssh_command`) on every network op (`ls-remote`, `fetch`, `set-head`). The
`ls-remote` precheck (stage 1c) uses the same key, so a key/connectivity problem
surfaces *before* the rescue mutates anything.

### Implementation Plan

#### Phase 1: Spec-optional migrate + main-worktree CWD resolution + absolutize
**Model:** sonnet
- `config.rs`: drop `Op::Migrate` from the requires-spec check; update doc comment.
- `cli.rs`: `--migrate` help says repospec optional.
- `lib.rs`: derive `flat` from `flat_from_cwd()` when no spec.
- `migrate.rs`: `flat_from_cwd()` resolving the enclosing repo's main worktree
  (reject bare container); `canonicalize` `flat` at the top of
  `migrate_flat_to_bare`.
- Tests: from cwd, from a subdirectory, from a legacy linked worktree (resolves
  to main checkout); `.git`-as-file/bare-container rejected; **relative-clonepath
  + >=2 worktrees no longer fails repair** (rough-edge #1 regression).

#### Phase 2: Preflight (rkvr + ssh key + connectivity) + rescue pass
**Model:** opus
- `list_worktrees()` parser capturing `path`, `branch`, `head` (and the trailing
  block + `detached` line).
- Preflight: require `rkvr` (bail if absent); resolve per-org key from `origin`;
  `git ls-remote origin` connectivity probe; detect git-ignored files.
- `rescue_work()`: detached-HEAD -> `wip/detached-<shortsha>`; stash each dirty
  worktree (`--include-untracked`); convert every `stash@{i}` to a prefix-safe,
  length-capped `wip/*` branch. Mid-merge/unmerged tree -> report and bail
  *before* mutating.
- Replace `ensure_clean`'s refuse-semantics with a post-rescue cleanliness
  assertion looping the **whole** worktree set *(finding 8)*.
- Tests: dirty main; dirty linked; non-empty stash; detached worktree with a
  unique commit; multi-worktree dirty (shared stash stack); **assert recovered
  file CONTENT, not just branch existence** *(finding 10)*; unmerged-tree bail;
  over-long stash message truncates *(finding 6)*; `wip/*` prefix-conflict
  *(finding 5)*.

#### Phase 3: Linked-worktree carry-over + recoverable orphan cleanup
**Model:** opus
- `recreate_linked_worktrees()` with `materialized` + `used_dirs` guards.
- Capture orphan external dirs up front; after a verified swap, `rkvr rmrf` each
  that still exists and is not under the new container.
- Extend `repair_worktrees` to the full worktree set.
- Tests: linked worktree carried + functional; linked worktree on the default
  branch skipped (no double-checkout fatal); orphan removed; rollback restores
  everything on an injected post-swap failure.

#### Phase 4: Summary, dropped-state counts, target + ignored-file notes, test fixups, docs
**Model:** sonnet
- `warn_dropped_state` with counts (custom hooks, HEAD reflog entries).
- `target_symlink()` detection + relocate-targets pointer; ignored-file list in
  the summary; `print_summary()` to stderr (wip branches, carried worktrees,
  removed orphans, ignored files, target note).
- **Rewrite `test_migrate_refuses_dirty_tree` / `test_migrate_refuses_nonempty_stash`**
  — they assert `.unwrap_err()`, but the new behavior auto-rescues and succeeds.
- Update `git-tools/CLAUDE.md` `--migrate` bullet. `otto ci` green.

## Alternatives Considered

### Alternative 1: Refuse on any dirty/stashed tree (status quo, extended)
- **Pros:** simplest; never auto-mutates branches.
- **Cons:** not "smooth" — the operator still does the work by hand.
- **Why not:** the explicit ask is to *handle* the issues; the additive rescue +
  `rkvr` backstop makes lossless auto-rescue safe.

### Alternative 2: `rkvr bkup` the whole repo up front
- **Pros:** one pristine snapshot.
- **Cons:** `rkvr bkup` is never harvested — permanent disk growth; the swap +
  `rkvr rmrf` already give recoverable rollback.
- **Why not:** user decision; redundant with the existing backstop.

### Alternative 3: Commit dirty work onto its own branch
- **Pros:** no extra `wip/*` branches.
- **Cons:** rewrites the user's real branches with a synthetic commit; needs a
  message; muddles authored vs in-flight work.
- **Why not:** quarantined `wip/*` rescue is cleaner and keeps the additive
  invariant.

### Alternative 4: Auto-copy git-ignored files into the new worktrees
- **Pros:** smoother for `.env`-style files.
- **Cons:** ignored payload is usually large build output (`target/`,
  `node_modules/`); copying it is slow and wrong, and any size/dir heuristic is
  fragile and unpredictable.
- **Why not:** user chose warn + recoverable.

## Technical Considerations

### Dependencies
- Internal: `bare::*`, `git::{run,output,slugify_branch,ssh_command,parse_repospec}`,
  `config::find_ssh_key_for_org`.
- External: `git` (required), `rkvr` (**required** — preflight-enforced).

### Verified git behavior (load-bearing)
- `git clone --bare <local-flat>` copies all of `refs/heads/*` (incl `wip/*` and
  local-only) and does **not** create `refs/stash`. Verified.
- A local clone hardlinks the whole object store, so a detached worktree's unique
  commit physically survives but is unreferenced/GC-eligible — hence the
  `wip/detached-<sha>` ref. Verified.
- `git worktree add <b>` where `<b>` is already checked out is **fatal** — hence
  the `materialized` guard. Verified.
- `slugify_branch` (`common/src/git/spec.rs:136`) emits `[a-z0-9-]+`, always a
  valid ref component; length is capped and collisions suffixed.
- `refs/heads/` enforces file-vs-directory exclusivity (`wip/foo` vs
  `wip/foo/bar`) — handled by prefix-aware naming.

### Security
- Persona invariant holds: worktrees stay under `~/repos/<org>/` so the
  `includeIf gitdir:~/repos/tatari-tv/` identity fires. Migration is in-place and
  cannot move a worktree out of an org prefix it is not already in; for a repo
  already correctly located the invariant is preserved. *(Panel finding 12: the
  only residual gap is that migrate does not warn about an already-misplaced repo
  — deferred as an optional preflight warning.)*
- Orphan removal canonicalizes and only removes paths **not** under the new
  container, via `rkvr rmrf`. No `rm -rf`; with `rkvr` now preflight-required,
  the non-recoverable `fs::remove_dir_all` fallback is dropped *(finding 7)*.

### Testing Strategy
- Integration tests in `clone/src/migrate/tests.rs` using `make_remote`/`make_flat`,
  extended with helpers for linked worktrees, dirty trees, stashes, and detached
  worktrees. Content-recovery assertions, not just ref existence.
- Each phase ends `otto ci`-green. The two refuse-tests are converted in Phase 4
  (a red `test_migrate_refuses_*` between Phase 2 and Phase 4 is *expected*).

### Rollout Plan
- Ship with `/shipit` (commit -> `bump` patch, synchronized -> push main + `v*`
  tag -> `otto install`). Main ungated. No data migration; existing bare
  containers untouched.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Failure after rescue mutates the source | Med | Med | Additive-rescue invariant + early preflight (rkvr/key/connectivity) before any mutation; documented recovery |
| Git-ignored files (`.env`) not carried over | High | Med | Detected + listed in summary; recoverable via `rkvr rcvr`; claim scoped to git-tracked |
| Surprise `wip/*` branches | High | Low | Summary lists every branch; quarantined under `wip/`; nothing lost |
| `rkvr` absent | Low | Med | Preflight requirement — bail before any delete |
| Double-checkout fatal on default-branch linked worktree | Med | Med | `materialized` skip + dedicated test |
| Detached unique commit GC'd later | Low | High | Rescued to `wip/detached-<sha>` |
| `wip/*` ref prefix conflict | Low | Med | Prefix-aware naming + suffix retry + `git branch`-failure fallthrough + test |
| Over-long stash-message slug | Low | Med | Length-capped `wip_branch_name` + test |
| Stash fatal on unmerged/mid-merge tree | Low | Med | Report + bail before mutating that tree |
| Submodule worktrees not populated | Med | Low | Documented non-goal; content preserved |
| Mid-series red `test_migrate_refuses_*` | High | Low | Documented; converted in Phase 4 |

## Open Questions
- [x] ~~Optional preflight warning when the repo is outside `~/repos/<org>/`~~ -
      **declined.** The user places repos wherever they choose; migrate has no
      business policing location.
- [x] ~~Worth a `--dry-run`?~~ - **implemented.** `clone --migrate --dry-run`
      runs the read-only preflight and prints the full plan (worktrees, rescues,
      carry-overs, removals, ignored files, target note) without mutating. This
      is the sanctioned exception to the "no `--dry-run` on opt-in destructive
      flags" rule: `--migrate` is a heavy, hard-to-preview one-shot.

## References
- Handoff punch list: `clone/docs/2026-06-26-migrate-rough-edges.md`
- Layout design: `clone/docs/design/2026-06-21-clone-bare-worktree.md` (+ notes)
- Cross-model panel review: Architect (Gemini) + Staff Engineer (Codex),
  2026-06-27 — findings 1–10 incorporated; 11–13 deferred with rationale.
- Prototype (reference only, not shipped): uncommitted edits to
  `clone/src/{migrate,lib,cli,config}.rs`
- Wrapper contract + tagging rules: `git-tools/CLAUDE.md`
