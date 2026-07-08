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
