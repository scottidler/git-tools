# Implementation Notes: Worktree-Savvy `clone` (Bare-Repo + Nested Worktrees)

Running record of how the implementation interprets or diverges from
`2026-06-21-clone-bare-worktree.md`. Append-only, one section per phase.

## Phase 1: Extract `lib.rs` + module split

### Design decisions
- Module layout — created `clone/src/{cli.rs, config.rs, lib.rs, main.rs}` plus
  per-module test files (`config/tests.rs`, `tests.rs`). `bare.rs` and
  `migrate.rs` are **deferred to their owning phases** (2 and 4) rather than
  created as empty stubs now — an empty module would be dead code, and
  `#[allow(dead_code)]` is banned by the Rust conventions. The design's Phase 1
  bullet lists the eventual module set; this phase lays down the shell/core
  split, and the bare/migrate modules arrive with their content.
- `find_ssh_key_for_org` moved into `config.rs` (`crate::config`) because the
  resolved `Config.ssh_key` is its only consumer; `Config::try_from(Cli)` calls
  it with `&spec.org`. The function still accepts an `org` or `org/repo` string
  (it splits on `/` and takes the leading component), so the moved-verbatim
  tests pass unchanged.
- `REMOTE_URLS` lives in `lib.rs` (`crate::REMOTE_URLS`) and is referenced from
  `cli.rs`'s `default_value` and from the transport helpers — single source.

### Deviations
- Replaced the process-global `std::env::set_current_dir` + `cwd: None` git
  invocation pattern with explicit `cwd: Some(repo)` arguments to
  `common::git::run`/`output` in `lib.rs::{run, update_existing_repo,
  clone_new_repo}`. Observable behavior is identical (same git commands against
  the same directories, same stdout destination path), but the core no longer
  mutates global process state, which is what makes `run` testable per the
  design's "extract a testable `lib.rs`" goal. This is a mechanism change, not a
  behavior change.

### Tradeoffs
- Kept `std::fs::remove_dir_all(&full_clone_path).ok()` in the empty-clone
  cleanup path verbatim (not routed through `rkvr`). Phase 1 is strictly
  behavior-preserving; the path deletes a directory `clone` itself just created
  moments earlier (a failed/empty clone), so the recoverability concern is low.
  Revisit only if a later phase touches this path.
- Kept the operational `eprintln!` notes (stash warning, verbose clone
  messages) in `lib.rs`. They write to stderr (not stdout), so the wrapper
  contract — stdout is only the destination path — is preserved.

### Open questions
- None.

## Phase 2: Bare-container setup + default worktree

### Design decisions
- `setup_bare_container(config: &Config) -> Result<PathBuf>` takes `&Config`
  only, dropping the design's redundant `spec: &RepoSpec` parameter — `Config`
  already carries `spec`, so a second `spec` arg would be the same data passed
  twice. (`bare.rs`)
- Introduced `transport.rs` with a single `clone_with_fallback(... extra: &[&str]
  ...)` primitive used by **both** the flat and bare paths. This replaces
  Phase 1's two near-identical `attempt_clone`/`attempt_clone_with_ssh`
  functions in `lib.rs`; the `--bare` flag is just an `extra` arg. The design's
  "reuse the existing SSH-key + SSH→HTTPS fallback for the --bare clone" is
  realized by this shared primitive.
- `Op` enum **deferred to Phase 3**. With only the clone operation (bare or flat
  chosen by `Layout`) in scope this phase, an `Op` enum would have a single
  constructed variant — dead code. It arrives when `--worktree` (Phase 3) adds a
  second operation.
- `default_branch(container, fallback)` (`bare.rs`) takes an explicit
  `Option<&str>` fallback (sourced from `clone.cfg` `[clone] default`) rather
  than reading config itself, keeping it a pure function of (container, cfg
  value). The richer fallback chain (HEAD → origin/HEAD → `remote set-head -a` →
  cfg default) is bare-setup-specific and intentionally distinct from
  `common::repo::info::default_branch` (the discovery-time variant that assumes
  a well-formed container).
- `Config.default_branch` reads `clone.cfg` `[clone] default` via a new
  `clone_cfg_value` helper; `find_ssh_key_for_org` was left untouched (exact
  original error behavior preserved) and reads the file independently. The few
  extra startup file reads are negligible.

### Deviations
- `run_bare` dispatch handles the three "Edge cases handled" rerun scenarios in
  this phase (not a later one), because flipping the default to bare is exactly
  what makes them reachable: existing bare container → idempotent
  `reconcile_container`; existing **flat** checkout → update in place + print the
  `--migrate` hint (never silently convert); else fresh setup. The commitless-
  remote case is handled in `ensure_default_worktree` (skip worktree add, return
  the container).

### Tradeoffs
- On a failed bare clone, `setup_bare_container` removes the container directory
  it pre-created (via `fs::remove_dir_all`) so a failed clone leaves no empty
  turd dir — matching the flat path's observable behavior. Consistent with the
  existing empty-clone cleanup in `clone_new_repo`.
- Fixed a latent test-isolation bug surfaced (not caused) by this phase: the
  `find_ssh_key_for_org` tests mutated the shared `CLONE_CFG` env var and a fixed
  `/tmp/clone_test_config` path with no isolation, so the `ini!` macro could
  panic on a file deleted mid-read by a parallel test. Serialized all
  env-touching config tests behind an `ENV_LOCK` mutex and switched to a unique
  `TempDir` with prior-value restore (per the Rust platform-path testing
  convention). Production `find_ssh_key_for_org` is unchanged — its `exists()`
  guard is correct; only the tests were unsafe.

### Open questions
- `clone.cfg` `default-layout: bare|flat` is now read (CLI `--flat` overrides
  it; default is `Bare`). The design left this as a proposed open question; it is
  implemented as proposed. The live `clone.cfg` template comment is updated in
  Phase 5.

## Phase 3: `--worktree` flag

### Design decisions
- Introduced `Op { Clone, AddWorktree(String) }` (deferred from Phase 2 to here,
  where the second operation appears) and made `Config.spec` `Option<RepoSpec>`
  (it can be `None` for `--worktree` run inside a container). `Cli.repospec`
  became `Option<String>` (no longer `required`); `arg_required_else_help` still
  shows help on a bare `clone`. `Config::try_from` validates: `Op::Clone`
  requires a spec; `--flat` + `--worktree` is rejected.
- `setup_bare_container` regained its `spec: &RepoSpec` parameter (dropped in
  Phase 2 as redundant) — now that `Config.spec` is `Option`, passing the spec
  explicitly is necessary, and this matches the design's API signature.
- Directory naming: the worktree directory is `slugify_branch(checked-out-branch)`
  in **all** cases. The design body is internally ambiguous ("slugify only the
  directory" vs "branch name with / → -"); I followed the worked example
  (`release/1.2` → `release-1-2/`), which requires full slugification (dots and
  slashes both collapse to hyphens), not just slash replacement. New branches use
  the slug as both branch and dir; existing branches keep their real name for
  checkout and only the dir is slugified. (`worktree.rs::add`)
- Container resolution from CWD uses `git rev-parse --git-common-dir` then
  `canonicalize` + parent-of-`.bare`, per the design. `canonicalize` resolves a
  relative `.bare` against CWD robustly.

### Deviations
- None.

### Tradeoffs
- Branch-source selection probes refs with `git rev-parse --verify --quiet
  refs/heads/<arg>` then `refs/remotes/origin/<arg>` (raw arg first, per the
  reviewer note about not locking out `feature/auth-fix`), falling through to a
  new slugified branch. Idempotency is by worktree-dir existence (a re-run `cd`s
  into the existing worktree rather than erroring).

### Open questions
- None.

## Phase 4: `--migrate`

### Design decisions
- `migrate_flat_to_bare(flat, default_fallback)` (`migrate.rs`) takes the flat
  path plus the `clone.cfg` default-branch fallback, rather than the design's
  bare `(flat)` — it needs the fallback for `bare::default_branch`. lib.rs
  computes `flat = clonepath/org/repo` from `config.spec` and threads
  `config.default_branch`. `--migrate` requires a repospec (matches the design's
  only shown form, `clone --migrate org/repo`).
- `Op::Migrate` added; `--worktree` + `--migrate` and `--flat` + `--migrate` are
  both rejected in `Config::try_from`.
- Reuses `bare::{write_git_pointer, fix_fetch_refspec, default_branch,
  add_worktree}`; `write_git_pointer` was promoted to `pub(crate)`.

### Deviations
- **Worktree-link repair after the swap (required for correctness; the design
  omits it).** The design creates worktrees in `<repo>.migrating` before the
  rename swap, but git worktree admin files store **absolute** paths recorded at
  the staging location, so the bare↔worktree links break the moment the
  container is renamed. I verified this empirically (a prototype: `git status`
  in the worktree fails `fatal: not a git repository` after the rename) and that
  `git worktree repair <new-abs-path>` run from the container fixes it. So after
  the `<repo>.migrating` → `<repo>` rename, `migrate.rs::repair_worktrees` runs
  `git worktree repair` with each worktree's new absolute path, then re-verifies.
  The `.git` pointer itself is relative (`gitdir: ./.bare`) and survives the
  rename untouched; only the per-worktree links need repair. Without this step,
  the design's "re-verify after swap" would fail on every migrate with a
  worktree.
- Added rollback on the two post-capture failure windows (swap-in failure →
  restore original from backup; post-swap verify failure → remove the half-built
  container and restore from backup), strengthening the design's "leaves both
  intact" guarantee through the swap itself.

### Tradeoffs
- Deletes (`<repo>.migrating` leftovers, the `<repo>.backup`) route through a
  `remove_dir` helper that prefers `rkvr rmrf` and falls back to
  `std::fs::remove_dir_all` + WARN when rkvr is absent (per the safety rule and
  `feedback-rust-deletes-via-rkvr`). Backup removal is best-effort (a warn, not a
  hard error) since the migration is already committed and verified at that
  point; a leftover `.backup` is harmless and recoverable.
- `warn_dropped_state` flags custom (non-`.sample`) `.git/hooks` and always warns
  that machine-local `.git/config` extras, alternates, and reflogs are not
  migrated, per the design's "stated, not left silent" requirement.

### Open questions
- None.

## Phase 5: `cd`/`z` navigation shim + docs

### Design decisions
- The navigation shim is a zsh **`chpwd` hook** (`_clone_chpwd_bare` appended to
  `chpwd_functions`), not a `cd` override. `chpwd` fires after *every* directory
  change - `cd`, `pushd`, and `z`/zoxide alike - which is exactly the "any way of
  arriving at the container" coverage the design's open question weighed. It
  ships in `shell-functions.sh` peer to the `clone` wrapper (the design's
  proposed location). Detection is `.bare/` dir + `.git` pointer file; the
  default branch comes from `git --git-dir=.bare symbolic-ref --short HEAD`. The
  one-level redirect into the worktree re-fires `chpwd`, which no-ops (the
  worktree is not a bare container), so it terminates. Verified live in zsh.
- `clone.cfg` `default-layout` template comment was added to the live config
  (`scottidler/dotfiles/HOME/.config/clone/clone.cfg`) - there is no separate
  template in the git-tools repo, so the user's actual config is the template.
  The edit is additive documentation only (a comment + a commented-out
  `default-layout = bare`); behavior is unchanged since `bare` is already the
  default. **This is an uncommitted change in the dotfiles repo** left for the
  user to review and commit (per the no-unauthorized-cross-repo-git-state rule).

### Deviations
- None.

### Tradeoffs
- The `chpwd` hook uses `builtin cd` for the redirect to avoid recursing through
  any user `cd` function, and guards re-registration with the
  `chpwd_functions[(I)...]` index test so re-sourcing `shell-functions.sh` does
  not stack duplicate hooks.

### Open questions
- None.
