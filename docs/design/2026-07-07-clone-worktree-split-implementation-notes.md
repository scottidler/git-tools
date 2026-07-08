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
