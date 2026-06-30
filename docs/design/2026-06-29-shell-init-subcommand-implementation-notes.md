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

## Phase 3: worktree emitter + pre-dispatch

### Design decisions

- `const ZSH: &str = concat!(...)` reuses the same `env!("GIT_DESCRIBE")` mechanism from the worktree crate's existing `build.rs` - no new build infrastructure was added -- `worktree/src/shell.rs:ZSH`.
- The emitted `worktree()` function uses a `case "$1" in` dispatch with `-*|shell-init)` for the passthrough branch and `*)` for the stdout-capture + cd branch - this exactly matches the design doc's "Emitted worktree() (zsh)" section, which differs structurally from `clone()`'s `if/else` form because worktree's flags (`--list`, `--prune`) must also pass through without triggering the capture path.
- Pre-dispatch in `main.rs` inserted before `Cli::parse()`, identical shape to clone's: inspect `std::env::args().skip(1)`, check for `"shell-init"`, emit and return -- `worktree/src/main.rs:main`. The positional/flag path below is byte-for-byte unchanged.
- `pub mod shell;` registered in `worktree/src/lib.rs` in alphabetical order between `prune` and `switch` -- `worktree/src/lib.rs`.
- Tests placed in `worktree/src/shell/tests.rs` with `#[cfg(test)] mod tests;` declared at the bottom of `shell.rs`, mirroring the Phase 2 pattern exactly.
- Added an explicit `zsh_script_has_flag_and_shell_init_passthrough_case` test asserting `-*|shell-init)` is present in the script body - this is the Phase 3 addition called out in the spec ("plus assert `-*|shell-init)` passthrough").

### Deviations

- None.

### Tradeoffs

- `case "$1" in` dispatch vs `if/else` form (clone's approach) - worktree uses `case` because it must simultaneously passthrough both flags (`-*`) and the `shell-init` literal; a combined `[[ "$1" == (-*|shell-init) ]]` zsh pattern would also work but the `case` form is what the design doc specifies and is more readable when the pattern set grows.
- Alphabetical ordering of `pub mod shell;` in `lib.rs` (between `prune` and `switch`) vs appending at the end - alphabetical ordering matches the existing style in `lib.rs` and keeps diffs minimal.

### Open questions

- None.
