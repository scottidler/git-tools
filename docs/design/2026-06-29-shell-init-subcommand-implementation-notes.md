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

## Phase 2: clone emitter + pre-dispatch

### Design decisions

- `const ZSH: &str = concat!(...)` built with `concat!` macro splicing `env!("GIT_DESCRIBE")` inline - this reuses the same build.rs env variable that `cli.rs` already uses for `--version`, so the header comment in the emitted script and the binary's `--version` output are always in sync. No new build mechanism was introduced -- `clone/src/shell.rs:ZSH`.
- Pre-dispatch in `main.rs` inspects `std::env::args().skip(1)` before `Cli::parse()`, consuming just enough args to check the first token and optionally the second -- `clone/src/main.rs:main`. The rest of the positional/flag path is byte-for-byte unchanged.
- `pub mod shell;` registered in `clone/src/lib.rs` so `init_script` is accessible from `main.rs` via `clone::shell::init_script` (the import is `use clone::{..., shell}`) -- `clone/src/lib.rs`.
- Tests placed in `clone/src/shell/tests.rs` with `#[cfg(test)] mod tests;` declared at the bottom of `shell.rs` -- matches the Rust 2018+ submodule convention used throughout the workspace.
- `zsh -n` syntax check in tests uses a runtime `Command::new("zsh")` check and skips (prints a message, returns) rather than failing when zsh is absent from PATH -- keeps CI green on environments without zsh while still exercising the check where it is available.

### Deviations

- None.

### Tradeoffs

- `concat!` with `env!("GIT_DESCRIBE")` for the ZSH const vs a `format!` at call time in `init_script` - `concat!` embeds the version at compile time (a `&'static str` constant), avoiding any runtime allocation or formatting. Since the version is always the build-time git-describe value, compile-time embedding is correct and cheaper. `format!` would allocate on every call for the same result.
- Putting `SUPPORTED` in `clone/src/shell.rs` (not in `common`) - per the design doc, each crate owns its supported list next to its bodies so the error message never claims a shell that crate does not emit. This was not deviated from.

### Open questions

- None.
