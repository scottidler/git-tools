# Design Document: Consolidate bare-container worktree primitives into `common`

**Author:** Scott Idler
**Date:** 2026-06-28
**Status:** In Review
**Review Passes Completed:** 5/5 (Rule of Five) + cross-model panel (Architect/Gemini, Staff Engineer/Codex) incorporated

## Summary

The "create a git worktree for a branch" logic is implemented three times across
two binary crates (`clone` and `worktree`), each a partial copy that has already
drifted. This doc proposes a single, fully-guarded worktree-add primitive in the
`common` crate that both tools call, with a collision-policy option so the
batch-recreation (`clone --migrate`) and interactive-single-add (`worktree`) use
cases share one implementation. Behavior is preserved for every realistic input;
the one deliberate change is that a slashed/spaced branch now yields the same
slugified directory from both tools (today `clone` would create a nested raw dir),
which also fixes a latent inconsistency.

## Problem Statement

### Background

`clone` shipped first and grew the bare-container + worktree layout
(`docs/design/2026-06-21-clone-bare-worktree.md`). The `worktree` tool was carved
out of `clone` later (shipped v0.2.13) to own live worktree navigation
(switch/create/list/prune). During that carve-out only the layout *detection*
helpers (`is_bare_container`, `default_branch`) were lifted into `common::bare`;
the worktree-*creation* logic was left forked between the two crates.

### Problem

The `git worktree add` logic exists in three places, each a thinner copy of the
next, and they have already diverged:

- **Canonical** - `worktree/src/switch.rs::switch` (+ `ensure_or_add`): a 3-case
  matrix (existing-local / remote-only / new-branch) with a slugified directory,
  an idempotency guard, and a slug-collision check that bails.
- **Thin copy A** - `clone/src/bare.rs::add_worktree` (line 127):
  `git worktree add <branch> <branch>` - directory == raw branch name, no guards.
- **Thin copy B** - `clone/src/migrate.rs::{add_default_worktree (743),
  add_named_worktree (768), recreate_linked_worktrees (779)}`: a parallel 3-way
  dispatch whose remote-tracking arm (`migrate.rs:748-754`) joins on the **raw**
  branch name as the directory (`container.join(branch)`), where `switch`
  slugifies the identical case (`switch.rs:32-41`).

The drift is a latent bug: a default branch containing a slash (rare but legal)
would produce a different directory layout from `clone --migrate` than from
`worktree`, and `clone`'s default-worktree add lacks the idempotency/collision
guards that the canonical path has. `ref_exists` is additionally copy-pasted
verbatim in three files (`switch.rs:100`, `migrate.rs:761`, `prune.rs:126`).

### Goals

- One worktree-add implementation in `common`, consumed by both `clone` and
  `worktree`.
- Preserve each consumer's externally-visible behavior (directory names, exit
  codes, idempotency) except where unifying it is a deliberate, documented fix.
- Eliminate the duplicated `ref_exists` helper.
- Keep `clone` free of any dependency on the `worktree` binary crate - the shared
  home is `common`.

### Non-Goals

- Folding `git worktree list` / `remove` / `prune` / `repair` into the shared
  primitive. Those are read/destroy/maintenance operations orthogonal to the
  add-fork; they stay where they are.
- Changing the `clone --migrate` rescue/preservation semantics (dirty-tree
  `wip/*` branches, unpushed-commit preservation, `rkvr` backup).
- Merging `clone`'s container *setup* (`setup_bare_container`,
  `reconcile_container`, `fix_fetch_refspec`) into `common`. Only the
  per-branch worktree-add is in scope.
- Any change to the `worktree` tool's CLI surface or the `worktree()` shell
  wrapper.

## Proposed Solution

### Overview

Add a low-level primitive `common::bare::add_worktree` that takes a fully-resolved
specification (branch name, source ref, collision policy) and performs the single
`git worktree add` with the idempotency/collision guard. The primitive derives the
directory itself (`slugify_branch(branch)`); the caller never passes one. Layer the
existing ref-probing case-selection (`switch`'s "is it local? remote? new?") as a
thin `resolve_and_add` helper on top, used by the `worktree` tool. `clone`'s call
sites - which already know the (branch, source) pair - call the low-level primitive
directly.

### Architecture

```
                 common::bare
                 ├── is_bare_container        (exists)
                 ├── default_branch           (exists)
                 ├── ref_exists               (NEW - single home)
                 ├── add_worktree(AddSpec)     (NEW - the guarded primitive)
                 └── resolve_and_add(...)      (NEW - switch's ref-probing layer)
                        ▲                              ▲
        ┌───────────────┘                              └───────────────┐
   clone (knows the tuple)                        worktree (probes refs)
   ├── bare.rs::ensure_default_worktree           └── switch.rs::switch
   └── migrate.rs::{add_default_worktree,             → resolve_and_add
        add_named_worktree, recreate_linked}          → add_worktree
        → add_worktree directly
```

`clone` does not depend on `worktree`; both depend only on `common`.

### Data Model

```rust
// common/src/bare.rs

/// Where a new worktree's content comes from.
pub enum Source<'a> {
    /// Check out an existing local branch as-is: `worktree add <dir> <branch>`.
    ExistingLocal,
    /// Create a local tracking branch from a remote ref:
    /// `worktree add -b <branch> --track <dir> <origin_ref>`.
    RemoteTracking { origin_ref: &'a str },
    /// Create a brand-new branch based on `base`:
    /// `worktree add -b <branch> <dir> <base>`.
    NewFrom { base: &'a str },
}

/// What to do when `branch` is already checked out, or the derived dir is taken.
pub enum Collision {
    /// Idempotent reuse: if `git worktree list` already has `branch` checked out
    /// anywhere, return that existing path (this is what makes a re-switch a no-op
    /// AND what keeps a legacy container whose worktree sits at a pre-slug raw
    /// path from triggering a fatal double-checkout). Otherwise, if the derived
    /// dir is occupied by an unrelated tree, bail. (interactive single-add -
    /// `worktree <branch>`)
    ReuseOrBail,
    /// Append a numeric suffix (`-1`, `-2`, …) until the dir is free, probed via
    /// `Path::exists()`. (batch recreation - `clone --migrate` linked worktrees)
    Uniquify,
}

pub struct AddSpec<'a> {
    /// The branch the worktree hosts. For `ExistingLocal`/`RemoteTracking` this is
    /// the real branch name (e.g. `feature/auth`); for `NewFrom` it is the slug
    /// that also names the new branch (e.g. `new-feature`).
    pub branch: &'a str,
    pub source: Source<'a>,
    pub collision: Collision,
}
```

The worktree **directory is not a field** - the primitive always derives it as
`slugify_branch(branch)` (then applies the collision policy, which may append a
suffix). This is correct for all three sources: a real branch like `feature/auth`
slugs to the dir `feature-auth` while keeping its real name in the git command,
and a `NewFrom` branch is already the slug so it maps to itself. Making the dir
underivable-by-the-caller is the whole point - it's what kept `clone` and
`worktree` from agreeing.

### API Design

```rust
// The guarded primitive: one `git worktree add`, honoring the collision policy.
// Returns the absolute path to the worktree (container.join(final_dir)).
pub fn add_worktree(container: &Path, spec: &AddSpec) -> Result<PathBuf>;

// Ref-probing convenience for the worktree tool: take a raw branch string,
// classify it (local / remote-only / new), slugify the dir, and add with
// Collision::ReuseOrBail. This IS the current `switch` body, relocated.
pub fn resolve_and_add(
    container: &Path,
    raw_branch: &str,
    default_branch: Option<&str>,
) -> Result<PathBuf>;

// Single home for the thrice-copied ref check.
pub fn ref_exists(container: &Path, refname: &str) -> bool;
```

The unified rule for every worktree the tools create: **the directory is
`slugify_branch(branch)`** (a slashed/spaced branch can't be a safe directory
name). Today only the `worktree` tool follows it; `clone`'s setup and migrate use
the raw branch as the dir. Call-site mapping after consolidation:

| Caller | Today | After (dir always `slug(branch)`) |
|--------|-------|-------|
| `worktree/src/switch.rs::switch` | own 3-case + `ensure_or_add` | `common::bare::resolve_and_add` |
| `clone/src/bare.rs::add_worktree` | `worktree add <b> <b>` (raw dir) | `add_worktree(ExistingLocal, ReuseOrBail)` |
| `migrate.rs::add_default_worktree` | own 3-way (raw dir) | `add_worktree(ExistingLocal or RemoteTracking, ReuseOrBail)` |
| `migrate.rs::add_named_worktree` | `worktree add <dir> <b>` | `add_worktree(ExistingLocal, Uniquify)` |
| `migrate.rs::recreate_linked` (`unique_dir`) | manual suffixing | `add_worktree(ExistingLocal, Uniquify)` |

Note: migrate rescues dirty/detached/stash state into `wip/*` branches *before*
recreating worktrees, so every `add_worktree` call it makes targets a real local
branch - `Source::ExistingLocal` always holds for migrate's recreated worktrees;
`RemoteTracking` is only the default-worktree arm when the flat repo had deleted
its local default.

### Implementation Plan

#### Phase 1: Land the `common::bare` primitive
**Model:** opus
- Add `Source`, `Collision`, `AddSpec`, `add_worktree`, `resolve_and_add`,
  `ref_exists` to `common::bare` (new `common/src/bare/` module dir; keep
  `common/src/bare.rs` as the 2018-style entry with `#[cfg(test)] mod tests;`).
- Port the six `worktree/src/switch/tests.rs` cases into
  `common/src/bare/tests.rs` as the primitive's spec (slugify, keep-real-name,
  remote-tracking, collision-bail, idempotent, empty-slug-rejected).
- No call sites rewired yet; independently committable.

#### Phase 2: Rewire the `worktree` tool
**Model:** sonnet
- `worktree/src/switch.rs::switch` delegates to `common::bare::resolve_and_add`;
  delete its private `ensure_or_add` and `ref_exists`.
- Confirm `worktree/src/switch/tests.rs` stays green (behavior unchanged).

#### Phase 3: Rewire `clone` (the behavior-changing phase)
**Model:** opus
- Point `clone/src/bare.rs::add_worktree` and
  `migrate.rs::{add_default_worktree, add_named_worktree, recreate_linked_worktrees}`
  at the primitive.
- Collision policy per site (verified against the code, not assumed): the
  **default** worktree is created separately (`migrate.rs:157`→`add_default_worktree`,
  and `bare.rs` setup) and uses `ReuseOrBail`; **both** `add_named_worktree` call
  sites (`migrate.rs:174` current branch, `:795` linked worktrees) are wrapped in
  `unique_dir` today, so **both** use `Collision::Uniquify`.
- Because `Uniquify` now does the suffixing via `Path::exists()` inside the
  primitive, **delete** the obsolete `used_dirs: HashSet` (seeded with the raw,
  unslugified default at `migrate.rs:161`) and **keep** the `materialized` branch
  set that guards against a fatal double-checkout of the same branch.
- Deliberate unification: every clone-created worktree dir becomes
  `slugify_branch(branch)`. migrate's current-branch/linked sites already slugify
  (`migrate.rs:173,794`), so this only actually changes the **default** worktree
  dir, and only when the default branch contains a slash/space (`main`/`master`
  are unchanged).
- Re-verify every `clone/src/bare/tests.rs` and `clone/src/migrate/tests.rs` case;
  update assertions only where the slug change is intended.

#### Phase 4: Cleanup + docs
**Model:** sonnet
- Repoint every `ref_exists` copy at `common::bare::ref_exists`: `switch.rs:100`
  and `migrate.rs:761` become dead and are deleted, but `prune.rs:126`'s copy is a
  **live dependency of prune** - it is *rewired*, not deleted.
- Grep for any stray `Command::new("git")`.
- `otto ci` green.
- Update `CLAUDE.md` "Common Crate Modules" + bare-layout section to document the
  consolidated primitive.

## Alternatives Considered

### Alternative 1: `clone` depends on the `worktree` crate
- **Description:** Make `worktree` a library crate and have `clone` call
  `worktree::switch`.
- **Pros:** No new `common` surface; reuses the canonical code directly.
- **Cons:** Inverts the natural dependency (a clone/migrate tool depending on a
  navigation tool); `switch` is raw-branch-string-shaped and doesn't fit migrate's
  "I already resolved the tuple" call sites; couples release/versioning awkwardly.
- **Why not chosen:** Wrong dependency direction; `common` is the shared home by
  the repo's own convention.

### Alternative 2: Leave the fork; just fix the slug drift in migrate
- **Description:** Patch `migrate.rs:754` to slugify and call it done.
- **Pros:** Minimal diff.
- **Cons:** Leaves three copies and the verbatim `ref_exists` triplication; the
  next change drifts again; doesn't address the missing idempotency guard.
- **Why not chosen:** Treats the symptom, not the fork.

### Alternative 3: A single `switch`-style entry point for both
- **Description:** One function taking a raw branch and always probing refs.
- **Pros:** Smallest API.
- **Cons:** migrate's call sites already know the (branch, source) pair and
  sometimes must Uniquify across a batch; forcing them through ref-probing is
  wrong and loses the batch-collision strategy.
- **Why not chosen:** The two consumers genuinely differ at the input boundary;
  the layered primitive + `resolve_and_add` fits both.

## Technical Considerations

### Dependencies
Internal only: `common::git` (run/output, `slugify_branch`), `common::bare`. No
new external crates. `clone` and `worktree` each already depend on `common`.

### Performance
No change - same number of `git worktree add` invocations; this is a code-locality
refactor.

### Security
Preserves the persona invariant: worktrees still created under
`~/repos/<org>/<repo>/` by the same callers (the primitive runs `git worktree add`
relative to the container it is handed and derives only the leaf dir name; it does
not choose container locations). Locked by
`clone/src/bare/tests.rs::test_persona_invariant`.

### Testing Strategy
- The six ported `switch` tests become the primitive's spec in
  `common/src/bare/tests.rs`.
- `worktree/src/switch/tests.rs` re-run unchanged after Phase 2 (delegation must
  be behavior-preserving).
- `clone/src/bare/tests.rs::test_setup_bare_container_layout` (asserts
  `container.join("main")`) and all `clone/src/migrate/tests.rs` re-verified in
  Phase 3; `main`/`feature` slug to themselves so these stay green. The only
  assertion changes allowed are dirs that the intended slug-unification moves.
- Add a test: a clone/migrate of a repo whose default (or recreated) branch name
  contains a slash now produces the slugified dir (the drift fix, asserted),
  matching what the `worktree` tool would produce.
- Add a test: empty-slug rejection for an **existing/remote** branch, not just a
  new branch - `slugify_branch` can return empty for any source (`spec.rs:136`).
- Add a compatibility test: a container whose default worktree sits at the *raw*
  pre-slug path (as old `clone` would have created for a slashed default) is
  reconciled idempotently - `ReuseOrBail` finds the branch via `git worktree list`
  and reuses the existing path rather than attempting a second checkout.

### Rollout Plan
Ships as a normal patch release via `/shipit` once all four phases are merged and
`otto ci` is green. No on-disk migration is needed for the common case
(`main`/`master` defaults are byte-for-byte unchanged). The one compatibility
concern is a container created by old `clone` with a *slashed* default branch: its
worktree lives at the raw path, so `ReuseOrBail` must locate the branch via
`git worktree list` (not the newly-derived slug path) to avoid a fatal
double-checkout - this is built into the primitive (see Data Model). Call the
slugified-default-dir change out in the release notes since the default worktree
path is the stdout the `clone`/`worktree` shell wrappers `cd` into (a documented
contract).

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| slug-unification changes the default worktree dir, which is the stdout the shell wrappers `cd` into (a contract) | Low (slashed defaults only) | Med | `main`/`master` unchanged; release-noted; covered by the slug + compatibility tests |
| Legacy container with a slashed-default worktree at the raw path → fatal double-checkout on reconcile | Low | High (if unhandled) | `ReuseOrBail` finds the branch via `git worktree list`, not the derived dir, and reuses the existing path; compatibility test asserts it |
| Collision policy mis-mapped (Bail where Uniquify needed) | Low | Med | Verified per call site: both `add_named_worktree` sites = Uniquify (wrapped in `unique_dir` today), default = ReuseOrBail; covered by migrate tests |
| Behavior drift during Phase 2 delegation | Low | Med | Phase 2 changes no behavior; the unchanged `switch` tests are the gate |
| Dry-run output diverges from a real `Uniquify` run (prints unsuffixed names) | Low | Low | `migrate.rs:306` dry-run is advisory; note it doesn't model suffixing, or share the planner |

## Edge Cases

- **Slug collision across recreated worktrees.** `feature/x` and a literal
  `feature-x` both slug to `feature-x`. `Uniquify` resolves the second to
  `feature-x-1` (preserving today's `unique_dir` behavior); `ReuseOrBail` would
  bail. This is exactly why migrate's batch path needs `Uniquify`.
- **Per-call-site collision policy (verified against the code).** `add_named_worktree`
  is called at `migrate.rs:174` (current branch) and `:795` (linked worktrees);
  **both** are wrapped in `unique_dir` today (`:173`, `:794`), so **both** use
  `Collision::Uniquify`. The default worktree is created on a separate path
  (`migrate.rs:157`→`add_default_worktree`) and uses `ReuseOrBail`. Assigning `:174`
  `ReuseOrBail` would make `clone --migrate` bail fatally on a slug collision with
  the default dir - so the mapping is fixed here, not left to guesswork.
- **Legacy raw-path default worktree.** A container created by old `clone` with a
  slashed default has its worktree at the raw path (`container/release/1.2`). After
  the change the tool derives `container/release-1-2`; `ReuseOrBail` must find the
  branch via `git worktree list` and reuse the raw path, never attempt a second
  `git worktree add` (git rejects an already-checked-out branch fatally).
- **Idempotency vs detached HEAD.** `ReuseOrBail` reuses a dir only when it is a
  real worktree hosting `branch`; a detached or mismatched dir bails. Migrate never
  hits this - it rescues detached state to a
  `wip/*` branch before adding.
- **Return path under `Uniquify`.** `add_worktree` returns the *final*
  `container.join(dir)` including any suffix, so migrate's link-repair and the
  `worktree()` wrapper's `cd` target are always the real created path.

## Open Questions
- [x] **Resolved:** `Collision::Uniquify` lives inside the primitive (the dir is
  derived there, so the suffixing strategy stays in one place).
- [x] **Resolved:** `resolve_and_add` lives in `common::bare` directly (it's
  small; no submodule).

## References
- `docs/design/2026-06-21-clone-bare-worktree.md` - the bare-worktree layout
- `docs/shakedown-v0.2.13.md` - the shakedown that surfaced the worktree tool
- `worktree/src/switch.rs` - the canonical implementation
- `clone/src/migrate.rs`, `clone/src/bare.rs` - the forked copies
- CLAUDE.md - "Common Crate Modules", bare-worktree layout, git-invocation rule
