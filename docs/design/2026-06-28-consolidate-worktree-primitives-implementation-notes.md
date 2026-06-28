# Implementation Notes: Consolidate worktree primitives into `common`

Design doc: `docs/design/2026-06-28-consolidate-worktree-primitives.md`

## Phase 1: Land the `common::bare` primitive

### Design decisions
- Kept `common/src/bare.rs` as the 2018-style entry point and put test bodies in
  `common/src/bare/tests.rs` (`#[cfg(test)] mod tests;`) - matches the repo's
  module style and the rust.md test-placement rule. Preserved the existing
  `is_bare_container` + `default_branch` + `symbolic_ref_short` verbatim.
- `add_worktree` derives the dir from `slugify_branch(spec.branch)` and applies
  the collision policy inside the primitive - `common::bare::add_worktree`/`build_add_args`
  - the dir is intentionally NOT a caller field, which is the whole point of the
  consolidation (callers can no longer disagree on the dir name).
- Empty-slug rejection lives at the TOP of `add_worktree`, before any source
  dispatch - `common::bare::add_worktree` - so it fires for every source
  (`ExistingLocal`/`RemoteTracking`/`NewFrom`), not just new branches. The doc
  called this out explicitly (`slugify_branch` can return empty for any source).
- `ReuseOrBail` resolves "is this branch already checked out?" by branch name via
  `git worktree list --porcelain` - `common::bare::worktree_path_for_branch` -
  NOT by the derived dir. This is the single rule that delivers both idempotency
  (re-switch is a no-op) and legacy-raw-path compatibility (a worktree at the
  pre-slug raw path is found and reused, never double-checked-out).
- `Uniquify` probes `<slug>`, `<slug>-1`, `<slug>-2`, … via `Path::exists()` -
  `common::bare::unique_dir` - and `add_worktree` returns the suffixed path so
  callers always get the real created path.
- `resolve_and_add` is the relocated `switch` body - `common::bare::resolve_and_add` -
  classifying local / remote-only / new and always using `Collision::ReuseOrBail`.
- Inlined a minimal porcelain parse in `worktree_path_for_branch` rather than
  reaching for `worktree/src/list.rs::parse`, because that parser lives in the
  `worktree` binary crate and `common` must not depend on it (Goal: keep `clone`
  free of any `worktree` dependency; the shared home is `common`).
- All git invocation routes through `common::git::run` / `common::git::output`;
  no `Command::new("git")` introduced.

### Deviations
- Slight test rename: the six ported `switch` cases are prefixed `test_resolve_*`
  / `test_slug_collision_*` rather than the original `test_switch_*`, since they
  now exercise `resolve_and_add`/`add_worktree` rather than `switch`. Behavior
  asserted is identical. Added the two new doc-specified tests
  (`test_add_worktree_empty_slug_rejected_for_existing_branch`,
  `test_reuse_finds_legacy_raw_path_by_branch`) plus a `Uniquify` suffix test
  (`test_uniquify_appends_suffix_on_collision`) to cover the batch path the doc
  introduces.

### Tradeoffs
- `build_add_args` returns `Vec<String>` (then maps to `&[&str]` for `git::run`)
  vs. building a borrowed `&[&str]` slice directly. Chose owned Strings because
  the arg vector's shape varies by source and the borrowed-slice form would
  require juggling temporaries with awkward lifetimes; the allocation is once per
  worktree-add (not hot).
- No call site is rewired in Phase 1 (per the phase boundary). The new surface is
  `pub`, so it is part of the public API and not dead code - nothing needed an
  `#[allow(dead_code)]`.

### Open questions
None.

## Phase 2: Rewire the `worktree` tool

### Design decisions
- `switch` is now a 3-line thin delegator: DEBUG entry log, then
  `bare::resolve_and_add(container, raw_branch, default_branch)` -
  `worktree/src/switch.rs::switch`. The public signature is unchanged
  so the caller in `lib.rs::run` required zero edits.
- Deleted `ensure_or_add` and the local `ref_exists` from `switch.rs`
  entirely; the primitive equivalents now live in `common::bare`. The
  `ref_exists` in `prune.rs` was NOT touched (Phase 4's job, per the
  phase boundary in the design doc).
- The switch tests retained their full coverage (all 5 existing cases).
  `switch` still has the same observable behavior from the test's
  perspective; the tests just exercise the delegation path now. Because
  the tests needed `git::output` directly and the old `switch.rs` had
  `use common::git;` (which `use super::*` brought in), the import was
  added explicitly to `switch/tests.rs` - `worktree/src/switch/tests.rs`.
- No assertion text was changed: the slug-collision message from
  `common::bare::add_worktree` contains `"slug collision"` (the
  substring the test checks), matching the design doc's note that
  `ReuseOrBail`'s message still contains that substring.

### Deviations
- None. The phase spec said to delegate `switch` to `resolve_and_add`,
  delete `ensure_or_add` and the local `ref_exists`, and keep the tests
  green. That is exactly what was done.

### Tradeoffs
- Kept all 5 existing switch tests rather than thinning to a single smoke
  test. The design doc permitted thinning ("you may thin them to a single
  smoke test"), but the tests run quickly and deleting them would remove
  end-to-end coverage of the delegation path for each of the three ref
  cases plus the slug-collision and idempotency invariants. Keeping them
  costs nothing and catches any future regression in `resolve_and_add`.

### Open questions
- None.

## Phase 3: Rewire `clone` (the behavior-changing phase)

### Design decisions
- `clone/src/bare.rs::add_worktree` now delegates to `common::bare::add_worktree`
  with `Source::ExistingLocal` + `Collision::ReuseOrBail`. The public signature
  (`(container, branch) -> Result<PathBuf>`) is unchanged so `ensure_default_worktree`
  and the migrate default arm keep calling it as before.
- `clone/src/bare.rs::ensure_default_worktree` had a pre-check on
  `container.join(&branch).is_dir()` that returned early before the worktree-add.
  DELETED that pre-check so the default-worktree path now flows entirely through
  the primitive's `ReuseOrBail`. This is requirement #4: the pre-check on the RAW
  branch dir would bypass the by-branch reuse and reintroduce the raw-path/slug-path
  split. Idempotency is preserved because `ReuseOrBail` locates an already-checked-out
  default via `git worktree list` and returns its path.
- `clone/src/migrate.rs::add_default_worktree` keeps its existing 3-arm dispatch
  (local / remote-tracking / bail) but both add-arms now route through a small local
  `add_worktree(container, branch, source, collision)` wrapper over
  `common::bare::add_worktree`, with `ReuseOrBail`. The `RemoteTracking { origin_ref }`
  arm replaces the hand-rolled `git worktree add -b --track` (the drift the doc called
  out, which joined the dir on the RAW branch name).
- Both `add_named_worktree` call sites use `Collision::Uniquify`:
  `clone/src/migrate.rs::migrate_flat_to_bare` (the current-branch site, formerly
  `:174`) and `clone/src/migrate.rs::recreate_linked_worktrees` (linked worktrees,
  formerly `:795`). Verified against the code: both were wrapped in the local
  `unique_dir` today, and `:174` MUST be `Uniquify` (not `ReuseOrBail`) because the
  current branch's slug can collide with the default dir and `ReuseOrBail` would bail
  fatally there.
- `clone/src/migrate.rs::ref_exists` (the local copy) is replaced by
  `bare::ref_exists` (re-exported from `common::bare` via `clone/src/bare.rs`).
  Phase 4 owns the broader `ref_exists` cleanup, but `add_default_worktree` needed a
  ref check and the `common::bare::ref_exists` primitive already exists from Phase 1,
  so the local copy was deleted here rather than left dead.

### Deviations
- Slug-unification (the deliberate behavior change): every clone-created worktree dir
  is now `slugify_branch(branch)`, derived inside the primitive. In practice only the
  DEFAULT worktree dir moves, and only when the default branch contains a slash/space
  (`main`/`master` slug to themselves). migrate's current-branch and linked sites
  already slugified, so their observable dirs are unchanged. Cited:
  `clone/src/bare.rs::add_worktree` and `clone/src/migrate.rs::add_default_worktree`.
- Removed the obsolete `used_dirs: HashSet` (seeded with the raw unslugified default,
  formerly `migrate.rs:160-161`) and the local `unique_dir` helper (formerly
  `migrate.rs:808`). The `Path::exists()`-probed suffixing now lives inside the
  primitive's `Collision::Uniquify` (`common::bare::unique_dir`), so the by-hand
  dir-name set in `clone/src/migrate.rs::migrate_flat_to_bare` and the
  `used_dirs: &mut HashSet` parameter on `recreate_linked_worktrees` were both
  dropped. The `materialized` branch set (the double-checkout guard) is KEPT.
- No existing test assertion was changed. `main`/`feature`/`side` slug to themselves,
  so `container.join("main")`, `flat.join("feature")`, `container.join("side")`, etc.
  all stayed green. `test_persona_invariant_under_org_prefix` stays green unchanged.

### Tradeoffs
- Kept `clone/src/bare.rs::add_worktree` as a thin `(container, branch)` wrapper rather
  than rewriting every caller to build an `AddSpec` inline. It is called from two
  places with identical (ExistingLocal, ReuseOrBail) intent, so the wrapper reads
  cleaner and keeps the migrate default arm's call site small.
- The migrate local `add_worktree(container, branch, source, collision)` wrapper takes
  `source`/`collision` explicitly rather than exposing the full `AddSpec` at each call
  site; it keeps the three migrate call sites short while still routing through the
  single primitive.

### Open questions
- None.

## Phase 4: Cleanup + docs

### Design decisions
- Rewired `worktree/src/prune.rs`'s local `ref_exists` (formerly at line 126) to call
  `bare::ref_exists(container, refname)` directly - `worktree/src/prune.rs::prune`. The
  local private function was deleted. The `use common::{bare, git};` import was already
  present so no new import was needed.
- Confirmed `common::bare::ref_exists` is now the sole definition in the workspace. The
  copies in `switch.rs` and `migrate.rs` were removed in Phases 2-3 respectively; prune's
  copy is removed here.
- `Command::new("git")` appears exactly once in the workspace: `common/src/git/run.rs`,
  which is the implementation of the `common::git::run`/`output` runners themselves.
  No production code outside that file hand-rolls a git command - the rule is satisfied.
- Updated `CLAUDE.md` "Common Crate Modules" section to document the full `common::bare`
  surface: `bare::is_bare_container`/`bare::default_branch` (existing), `bare::ref_exists`
  (the single home for the check), `bare::add_worktree(container, &AddSpec)` with
  `Source`/`Collision` (the guarded primitive shared by `clone` and `worktree`), and
  `bare::resolve_and_add` (the ref-probing layer used by the `worktree` tool). The
  description makes explicit that `clone`'s call sites call `add_worktree` directly while
  `worktree` goes through `resolve_and_add`, so the doc no longer implies the logic is
  forked.
- Design doc status flipped from `In Review` to `Implemented`.

### Deviations
- None. The phase spec was followed exactly: rewire prune's `ref_exists`, grep for stray
  `Command::new("git")` (none found outside the runner), update CLAUDE.md, green CI.

### Tradeoffs
- Deleted the local `ref_exists` from `prune.rs` rather than leaving it as an alias for
  `bare::ref_exists`. Keeping a local alias would be dead weight; one call site, one
  function is simpler.

### Open questions
- None.

## Post-implementation audit follow-up

The cross-model review-panel (Architect/Gemini, Staff Engineer/Codex) audited the
committed implementation in Mode 2. The reviewers split; the Staff Engineer found a
real must-fix that the Architect missed, verified independently against the code.

### Must-fix (fixed here)
- `clone/src/migrate.rs::migrate_flat_to_bare` created the default-branch worktree via
  the primitive at the SLUG dir (`add_default_worktree` returns `worktree_paths[0]`), but
  then verified and swapped against the RAW default name: `verify(&migrating.join(&default))`
  and `final_worktree = flat.join(&default)`. For a slashed/dotted/mixed-case default
  (e.g. `release/1.2` -> dir `release-1-2`), those joins point at a non-existent path, so
  `verify` bailed and the WHOLE migration rolled back - exactly the slashed-default rollout
  case the doc flagged. `main`/`master` were unaffected (slug == raw), which is why every
  existing test stayed green and the Phase 3 slug test (slashed LINKED worktree) did not
  catch it.
- Fix: derive `default_dir = git::slugify_branch(&default)` once and use it for both
  filesystem joins (the raw `default` is still used for the `symbolic-ref HEAD` ref and the
  `materialized` branch set, which are branch names, not paths).
- Regression test added: `migrate::tests::test_migrate_slugifies_slashed_default_worktree_dir`
  - a remote whose default is `release/1.2` migrates successfully to a `release-1-2` worktree.
  Verified the test FAILS against the pre-fix code (migration rolls back) and passes after.

### Notes-accuracy corrections (from the audit)
- The earlier claim that "only slashed/spaced defaults move" is too narrow: `slugify_branch`
  lowercases and collapses ANY non-alphanumeric run, so dotted (`release.1`) and mixed-case
  default branch names also change dir. `main`/`master`/`feature`/`side` are genuinely
  unchanged. This widens (slightly) the blast radius noted in Phase 3.
- The Phase 4 `Command::new("git")` note said "exactly once in the workspace." Accurate for
  PRODUCTION code (the sole occurrence is the runner at `common/src/git/run.rs`), but
  test/other-crate occurrences exist (e.g. `ls-owners`, `ls-stale-prs`); the grep claim
  should have been scoped to `clone`/`worktree`/`common::bare` production paths.
