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
