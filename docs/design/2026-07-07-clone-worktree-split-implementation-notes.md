# Implementation Notes: clone/worktree architectural split

Design doc: `docs/design/2026-07-07-clone-worktree-split.md`

## Phase 1: Extract transport + config reader to `common`

### Design decisions
- `common::transport` carries `clone_with_fallback`, `try_clone`, and `REMOTE_URLS`
  moved verbatim from `clone/src/transport.rs` and `clone/src/lib.rs:21` — `common/src/transport.rs::clone_with_fallback` /
  `try_clone` — this is the shared clone primitive both `clone` (flat + bare) and
  the future `worktree init`/`migrate` (Phase 2) will call, so it has to live
  where neither binary owns the other.
- `common::config` carries `find_ssh_key_for_org` and `clone_cfg_value` moved
  verbatim (still INI, still `$CLONE_CFG`/`~/.config/clone/clone.cfg`) —
  `common/src/config.rs` — Phase 5 converts this to YAML at a shared XDG path;
  Phase 1 only relocates the reader, unchanged, per the doc's explicit
  "verbatim" instruction.
- `clone/src/transport.rs` is deleted outright (not left as a re-export shim) —
  `clone/src/lib.rs`, `clone/src/bare.rs` — the doc's Phase 3 bullet list still
  names `transport.rs` for deletion, but doing it now in Phase 1 satisfies the
  Phase 1 success criterion ("no transport/`ini` symbol defined in `clone` that
  isn't re-exported from `common`") immediately rather than deferring a
  half-finished state to Phase 3. `REMOTE_URLS` is kept reachable from `clone`
  via `pub use common::transport::REMOTE_URLS;` in `lib.rs` so `cli.rs`'s
  `use crate::REMOTE_URLS;` (the clap `default_value`) needed zero changes.
- `clone/src/migrate.rs::ssh_env_for_origin` now calls
  `common::config::find_ssh_key_for_org` directly instead of routing through
  `crate::config::find_ssh_key_for_org` — matches the doc's explicit rewire
  target (`migrate.rs:403`).
- `clone/src/config.rs`'s `TryFrom<Cli> for Config` now calls the imported
  `common::config::{clone_cfg_value, find_ssh_key_for_org}` in place of its own
  (now-deleted) copies; no `pub` re-export was added since nothing outside
  `clone::config` calls these through `clone::config` anymore (`migrate.rs` was
  the only such caller, and it was rewired to call `common::config` directly).
- Relocated unit tests moved with their symbols (per this workflow's
  "tests move with their symbols" rule): `test_find_ssh_key_*` and
  `test_remote_urls_constant` now live in `common/src/config/tests.rs` and
  `common/src/transport/tests.rs`; `clone/src/config/tests.rs` keeps only the
  clone-specific `Layout`/`Op`/CLI-validation tests, and `clone/src/tests.rs`
  was deleted (it held only the relocated `REMOTE_URLS` test).
- `cargo add ini` (not a hand-edited `Cargo.toml` line) added `ini = "1.3.0"`
  to `common`; `cargo remove ini` dropped it from `clone`'s direct
  dependencies — `clone` now reaches `ini` only transitively through `common`
  (confirmed via `cargo tree -p clone -i ini`).

### Deviations
- Deleted `clone/src/transport.rs` in Phase 1 rather than Phase 3 as the plan's
  file list implies. Same effect at the correct seam: the doc's own success
  criterion for Phase 1 ("no transport/`ini` symbol defined in `clone` that
  isn't re-exported from `common`") is unmet if the file survives with dead
  code, and Phase 3's re-listing of `transport.rs` for deletion is then simply
  a no-op confirmation, not a contradiction.

### Tradeoffs
- Re-exporting `REMOTE_URLS` from `clone::lib` (`pub use
  common::transport::REMOTE_URLS;`) vs. rewriting `clone/src/cli.rs` to
  reference `common::transport::REMOTE_URLS` directly: chose the re-export to
  keep `cli.rs` byte-identical (zero-risk for a behavior-neutral phase) and
  because a `pub use` re-export is explicitly allowed by the phase's own
  success criterion.
- Kept `common::config`'s reader INI-only and unchanged (no YAML support, no
  `deny_unknown_fields`, no XDG path) even though the design doc's Data Model
  section describes the eventual YAML schema. That schema and fail-closed
  semantics are explicitly Phase 5 work; doing it now would be scope creep
  into a later phase's success criteria.

### Open questions
- None.

## Phase 3: Strip clone to flat-only

### Design decisions
- Deleted `clone/src/{bare,migrate,flatten}.rs` and their `*/tests.rs` outright
  (`git rm`) — `clone/src/bare.rs`, `clone/src/migrate.rs`, `clone/src/flatten.rs`
  and their test submodules — Phase 2 already ported the e2e coverage to
  `worktree/tests/lifecycle_tests.rs` and the unit tests to
  `worktree/src/{bare,migrate,flatten}/tests.rs`, so clone's copies were
  confirmed-dead per the doc's copy-in-P2/strip-in-P3 rule; `transport.rs` was
  already gone (Phase 1), matching the task's note.
- Collapsed `clone::config::Op` to a single `Clone` variant and removed the
  `Layout` enum, `resolve_layout`, `run_bare`, `is_flat_clone` — `clone/src/
  config.rs`, `clone/src/lib.rs::run`/`run_clone` — `run_clone` now always
  calls `clone_or_update_flat` directly; no dispatch on layout remains.
- Removed `--bare`/`--migrate`/`--flatten`/`--dry-run` from `Cli`
  (`clone/src/cli.rs:37-62` per the task's line reference) and every
  combinability check in `Config::try_from` that referenced them
  (`--bare`+`--flat`, `--bare`+`--migrate`, `--flatten`+`--migrate`,
  `--dry-run` validity, etc.) — `clone/src/config.rs::TryFrom<Cli>` — the only
  remaining validation is "a repospec is required" since `Op::Clone` is now the
  sole operation.
- Removed the `default_branch: Option<String>` field from `Config` — it was
  read via `clone_cfg_value("default")` and consumed only by the deleted
  `migrate`/`flatten`/`bare` code paths (confirmed via grep before removing);
  nothing in the flat-clone path (`clone_or_update_flat`/`clone_new_repo`/
  `update_existing_repo`) ever read it.
- `--flat` and `--versioning` are UNTOUCHED per the task's explicit
  instruction — `clone/src/cli.rs` — `--flat` keeps its "retained as a no-op
  alias" help text.
- Rewrote `cli.rs`'s `after_help` to drop the `--bare`/`--migrate`/`--flatten`/
  `clone.cfg default-layout` prose and point users at `worktree init`/
  `worktree migrate`/`worktree flatten` instead — `clone/src/cli.rs` — so a
  user who reads `clone --help` after the flags vanish is told where the
  functionality moved, not left with dangling references to dead flags.
- Updated `clone/src/shell.rs`'s emitted-script header comment from
  "smart git clone (bare-worktree layout)" to "smart git clone (flat
  checkout)" per the task instruction — `clone/src/shell.rs::ZSH`.
- Rewrote `clone/src/config/tests.rs` from scratch: deleted every
  `resolve_layout`/`--bare`/`--migrate`/`--flatten`/`--dry-run` combinability
  test (the fields/flags they exercised no longer exist) and added three
  tests covering what remains — `test_config_requires_repospec` (error path),
  `test_config_from_valid_cli_produces_clone_op` (happy path), and
  `test_config_flat_flag_is_accepted_as_no_op_alias` (confirms `--flat` still
  parses and behaves as a no-op) — per the "every public function gets a
  happy-path and an error/edge case" rule.
- Deleted `clone/tests/lifecycle_tests.rs` outright (the whole file was
  `--migrate`/`--flatten`/`--bare` e2e coverage, already ported to `worktree/
  tests/lifecycle_tests.rs` in Phase 2) and replaced `integration_tests.rs`'s
  single `test_clone_bare_opt_in_layout` with three new tests —
  `test_clone_bare_flag_is_now_an_unknown_argument`,
  `test_clone_migrate_flag_is_now_an_unknown_argument`,
  `test_clone_flatten_flag_is_now_an_unknown_argument` — that directly assert
  the Phase 3 success criterion (each flag now exits non-zero with an
  unrecognized-argument error). These do not touch the network (clap rejects
  the unknown flag before any git invocation), so they run in every
  environment.
- Removed the now-fully-unused `tempfile` dev-dependency from `clone/Cargo.toml`
  (`cargo remove tempfile -p clone --dev`) — it was pulled in only by the
  deleted `bare`/`migrate`/`flatten` test modules; grep confirmed no remaining
  `clone` source or test references it after the strip.

### Deviations
- None. The task's file list said `clone/src/{bare,migrate,flatten,transport}.rs`
  but `transport.rs` was already deleted in Phase 1 (confirmed absent before
  starting, consistent with the Phase 1 notes' own documented deviation); this
  phase simply had nothing left to delete there.

### Tradeoffs
- Kept `Op` as a single-variant enum (`Op::Clone`) rather than collapsing it
  away entirely — the task's instruction was scoped to removing
  `Op::Migrate`/`Op::Flatten`, not the `Op` type itself; keeping the type (even
  at one variant) is the more surgical, literal reading of the phase and avoids
  touching `lib.rs::run`'s match-based dispatch shape for a phase explicitly
  bounded to flag/module removal.
- Wrote three separate unknown-argument tests (`--bare`/`--migrate`/`--flatten`)
  instead of one parametrized test — matches this crate's existing
  one-assertion-per-test style in `integration_tests.rs` and keeps each
  failure's test name self-diagnosing (a table-driven variant would report
  a bare "test #2 failed").

### Open questions
- None.

## Phase 4: Shell-init cd-on-init for worktree verbs

### Design decisions
- Confirmed (did not change) that `init`/`migrate`/`flatten` already land in the
  `worktree()` wrapper's capture-and-`cd` branch — `worktree/src/shell.rs`'s
  `case "$1" in -*|shell-init) ... ; *) ...cd... ;; esac` — because the three
  verbs are non-`-*` `argv[1]` tokens (pre-clap dispatched in
  `worktree/src/main.rs`), the existing blanket `*)` arm already routes them
  through capture-and-cd with zero wrapper changes needed; this phase adds
  coverage (`tests/shell-functions.zsh`) rather than code here.
- Added `Outcome::Previewed(PathBuf)` — `worktree/src/lib.rs::Outcome`,
  `worktree/src/lib.rs::run` (the `Op::Migrate`/`Op::Flatten` arms) — a new
  variant distinct from `Outcome::Switched`, returned only when `config.dry_run`
  is true. `worktree/src/main.rs::dispatch` matches it with an empty arm (`Outcome::
  Previewed(_path) => {}`), so `--dry-run` prints NOTHING to stdout. This is the
  correct seam: `migrate::dry_run`/`flatten::dry_run` (`worktree/src/migrate.rs`,
  `worktree/src/flatten.rs`) already wrote their human-readable preview to stderr
  via `eprintln!` since Phase 2 — the only bug was `main.rs` echoing the returned
  path to stdout for EVERY `Outcome::Switched`, dry-run included. Fixing the
  print boundary (not the `dry_run` functions' return values, which unit tests in
  `worktree/src/migrate/tests.rs`/`worktree/src/flatten/tests.rs` already assert
  equal the target path) is the minimal, correct-seam fix.
- Extended `tests/shell-functions.zsh` with a new "worktree() acquisition verbs"
  section: cd-on-`init`/`migrate`/`flatten` cases (stub returns a real dir, rc 0
  -> `$PWD` becomes `$dest`), an `init` non-zero-exit case (bail before cd, same
  contract as the branch form), and dry-run no-cd cases for both `migrate
  --dry-run` and `flatten --dry-run` (stub returns EMPTY stdout with rc 0 ->
  the wrapper's existing `[[ -z "$dest" ]]` guard fires, returns 1, stays in
  `$home`). Matches the existing stub-harness pattern (`STUB_OUT`/`STUB_RC` env
  vars, `builtin cd`/`check` helpers) rather than inventing a new harness.
- Extended `worktree/tests/lifecycle_tests.rs::test_e2e_flatten_dry_run_makes_no_changes`
  (an existing e2e test driving the BUILT BINARY, not the shell wrapper) with two
  new assertions: `dry.stdout.is_empty()` and `dry.stderr` contains `"DRY RUN"` —
  this locks the binary's own stdout/stderr contract independently of the shell
  wrapper, so a future regression is caught even if someone tests the binary
  directly without `eval`-ing the emitted function.

### Deviations
- None. The phase's file list (shell.rs unchanged, lib.rs/main.rs for the
  dry-run fix, tests/shell-functions.zsh for coverage) was followed exactly;
  the one seam correction (fixing `main.rs`'s print boundary via a new `Outcome`
  variant, rather than touching `migrate.rs`/`flatten.rs`'s `dry_run` return
  values) is "same effect, correct seam", not a deviation from the doc's intent
  — the doc's own diagnosis text ("flatten.rs ... currently return the
  container/target path on dry-run ... which would otherwise cd you to the
  container root") identifies the RETURNED PATH reaching the wrapper as the bug,
  and `Outcome::Previewed` severs exactly that path from stdout without
  requiring `dry_run` to lie about what it did (it still returns the
  would-be target, useful to a future caller or test).

### Tradeoffs
- A new `Outcome::Previewed` variant vs. reusing `Outcome::Switched` with a
  `bool` flag or an `Option<PathBuf>` — chose the distinct variant because
  `Outcome` is matched exhaustively in exactly one place (`main.rs::dispatch`),
  a `Previewed` variant self-documents "dry-run, print nothing" at the type
  level (searchable, can't be silently defaulted the wrong way in a future
  match arm), and it costs nothing since nothing outside `lib.rs`/`main.rs`
  matches on `Outcome::Switched` today (confirmed via grep, cited above).
- Extended the existing `test_e2e_flatten_dry_run_makes_no_changes` in place
  rather than adding a fresh `test_e2e_migrate_dry_run_stdout_is_empty` for
  symmetry — `worktree migrate --dry-run`'s underlying `dry_run` function
  already has dedicated unit coverage (`worktree/src/migrate/tests.rs::
  test_dry_run_makes_no_changes`), and the phase's shell-functions.zsh coverage
  exercises `migrate --dry-run` and `flatten --dry-run` identically at the
  wrapper level; a second binary-level e2e test for `migrate` would duplicate
  the `flatten` one without adding a new assertion class, so it was left out to
  avoid gold-plating this phase.

### Open questions
- None.

## Phase 2: Relocate bare/migrate/flatten into worktree + add verbs

### Design decisions
- Merged the relocated acquisition primitives into the EXISTING
  `worktree::bare` module rather than a new file — `worktree/src/bare.rs` —
  the module already owned `resolve_container_from_cwd` and re-exported
  `common::bare`, so `setup_bare_container`/`reconcile_container`/
  `fix_fetch_refspec`/`ensure_default_worktree`/`link_upstreams`/`add_worktree`
  land beside it and the re-export set gained `ref_exists` (migrate/flatten
  reach it through `crate::bare`).
- Introduced `AcquireArgs` as the scalar acquisition input replacing the
  clone-shaped `Config` — `worktree/src/bare.rs::AcquireArgs` — carrying
  `clonepath/remote/mirrorpath/ssh_key/verbose` PLUS `default_branch` (see
  Deviations). `setup_bare_container`/`reconcile_container` now take
  `&AcquireArgs`, never a clone `Config`.
- `migrate.rs`/`flatten.rs` relocated with their bodies intact — their public
  functions already took scalar args (`flat/container: &Path`,
  `default_fallback: Option<&str>`), never a clone `Config`, so the only
  adaptation was `use crate::bare` now resolving to `worktree::bare` and
  user-facing strings retargeted from `clone --migrate`/`--flatten`/`.cfg` to
  `worktree migrate`/`worktree flatten`/"config" (`migrate.rs::flat_from_dir`,
  `migrate.rs::rescue_work`, `flatten.rs::container_from_dir`, plus doc/`origin_
  default_branch`/`remote_default_branch` comments) — "names tell the truth".
- New `worktree::init` performs fresh acquisition and dispatches on target state
  (fresh setup / reconcile existing bare / update-in-place + hint on an existing
  flat clone), mirroring clone's old `run_bare` — `worktree/src/init.rs::init`;
  `update_flat_in_place` is the relocated `clone::lib::update_existing_repo`
  logic (untracked-guard + auto-stash + pull), checking out `HEAD` since `init`
  carries no revision arg.
- Split `run` so acquisition verbs never resolve an enclosing container —
  `worktree/src/lib.rs::run` matches `Init`/`Migrate`/`Flatten` (by spec, any
  cwd) and only the no-spec conversions + `switch`/`list`/`prune`/`pick` fall
  through to `run_local`, which does the `resolve_container_from_cwd()` that used
  to run unconditionally.
- Pre-clap dispatch extended to intercept `init`/`migrate`/`flatten` as
  `argv[1]` — `worktree/src/main.rs` — each handed to its own clap parser
  (`InitCli`/`MigrateCli`/`FlattenCli` in `cli.rs`) via
  `parse_from(std::env::args().skip(1))`: the verb token fills clap's binary-name
  slot and `#[command(name = "worktree <verb>")]` keeps `--help` usage honest, so
  `worktree init --help` prints usage and exits 0 (verified by an e2e test).
- `Op` gained `Init(RepoSpec)`/`Migrate(Option<RepoSpec>)`/`Flatten(Option<
  RepoSpec>)`; `Config` gained the acquisition fields + `dry_run`, built via
  `TryFrom<{Init,Migrate,Flatten}Cli>` (`worktree/src/config.rs`). `ssh_key` and
  the `default_branch` fallback are resolved from `common::config` (INI reader,
  unchanged this phase), NOT flags, matching the doc.
- Kept the blanket `-*` passthrough in `worktree/src/shell.rs` untouched (the
  DIVERGENCE from reviewer finding 3): verbs are non-`-*` so they land in the
  capture-and-cd branch; `--list`/`--prune` keep passing through with no cd.
- clone's bare/migrate/flatten and their tests are UNCHANGED this phase (copy in
  P2, strip in P3, per Resolved Decisions), so both crates stay green.

### Deviations
- `AcquireArgs` carries a sixth field `default_branch` beyond the doc's
  five-field list (`clonepath/remote/mirrorpath/ssh_key/verbose`) —
  `ensure_default_worktree` needs the fallback branch, so carrying it on the
  struct keeps the acquisition self-contained instead of threading a sixth
  positional arg. Same effect, correct seam.
- Relocated migrate/flatten tests were copied VERBATIM from clone
  (`worktree/src/{migrate,flatten}/tests.rs`) rather than "moved": Phase 3 strips
  clone's copies, so during Phase 2 both crates carry them to stay green (the
  doc's copy-in-P2/strip-in-P3 rule). The bare tests could not be copied
  verbatim — they were adapted to build `AcquireArgs` instead of a clone
  `Config` (`worktree/src/bare/tests.rs`).
- `worktree migrate`/`flatten` `--dry-run` still returns the target path (printed
  to stdout via `Outcome::Switched`), matching clone's current behavior. The
  design's Phase 4 changes dry-run to empty-stdout/stderr-only so the wrapper
  skips the `cd`; that is explicitly Phase 4 work, not done here.
- Main `worktree` `Cli` `after_help` was NOT updated to advertise the new verbs
  — `--help`/`after_help` true-up is assigned to Phase 5; leaving it avoids
  later-phase scope creep.

### Tradeoffs
- One flat `Config` carrying acquisition fields that the local ops
  (`pick`/`list`/`prune`/`switch`) ignore, vs. per-op config types — chose the
  single struct (a `Config::local` constructor fills benign defaults) to match
  clone's existing shape and keep `run`'s signature stable; the acquisition
  fields are inert for local ops.
- `init` reuses a full port of clone's `update_existing_repo` for the
  existing-flat-clone case vs. a minimal fetch-only update — chose the faithful
  port (untracked-guard + stash + pull) so "never clobbering" holds exactly as
  `clone --bare` did, at the cost of more relocated code that Phase 3 leaves on
  clone until stripped.

### Open questions
- None.
