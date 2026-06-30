## Phase 1: Shared common::shell scaffolding

### Design decisions

- `unsupported` returns `eyre::Report` (not `eyre::Result`) - the function only ever constructs an error, never a success value, so `Report` is the honest return type without wrapping in a spurious `Err(...)` at every call site. Callers use `Err(common::shell::unsupported(...))` -- `common/src/shell.rs:unsupported`.
- Test file placed at `common/src/shell/tests.rs` with `#[cfg(test)] mod tests;` declared in `shell.rs` -- matches the Rust 2018+ submodule convention used by `bare.rs` / `bare/tests.rs` throughout the workspace.
- Five tests written: happy-path naming, bad-shell echo, supported-set listing, single-item set, and empty set -- the empty set covers the degenerate "no shells supported yet" state that is valid at construction time.

### Deviations

- None.

### Tradeoffs

- `eyre::Report` vs `String` as return type -- `Report` keeps the error in the eyre ecosystem (chainable with `.wrap_err()`), while a bare `String` would lose context on the way up. `Report` chosen; callers in Phases 2-3 wrap it with `Err(...)` which is idiomatic.
- Single-line `debug!` / `eyre!` calls (rustfmt expanded them to multi-line in the first draft; the formatter collapsed them back) -- accepted the single-line form to stay under 100 chars and match the formatter's output.

### Open questions

- None.
