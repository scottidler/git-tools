# git-tools

A Rust workspace of CLI tools for git repository discovery, analysis, and management.

## Workspace Structure

- **`common/`** - Shared library with git URL parsing, repo discovery, language detection, transport,
  shared config reading, and parallel execution
- **`clone/`** - Acquisition only: clone repos from various spec formats (org/repo, SSH, HTTPS), always
  a flat checkout. Carries no bare-container/layout-conversion code.
- **`worktree/`** - Owns the entire bare-container lifecycle: `init` (fresh bare acquisition), `migrate`
  (flat -> bare), `flatten` (bare -> flat), plus the original day-2 switch/list/prune/pick inside an
  existing container
- **`ls-git-repos/`** - Recursively discover local git repos with language filtering
- **`ls-github-repos/`** - List GitHub org/user repos via API with language filtering
- **`ls-owners/`** - Detect CODEOWNERS files and identify un-owned code paths
- **`ls-stale-branches/`** - Find abandoned branches (no commits for N days)
- **`ls-stale-prs/`** - Find stale pull requests
- **`reposlug/`** - Extract owner/repo slug from git remote

## Build & Test

```bash
otto ci          # Full pipeline: lint + check + test
otto build       # Release build
otto install     # Install all binaries to ~/.cargo/bin
cargo test --workspace  # Run all tests
```

## Install & Wiring

Two separate installers are involved. Don't conflate them.

### 1. Rust binaries → `otto install`

```bash
otto install     # cargo install --path each workspace member into ~/.cargo/bin
```

The `.otto.yml` `install` task loops over `*/Cargo.toml` and runs `cargo install --path <dir>`
for every workspace member, replacing the binaries in `~/.cargo/bin`. This is the only
supported way to install the whole workspace locally.

- Releasing is done with `/shipit`: commit → `bump` (patch by default; `0.x.y` synchronized
  across all crates) → push `main` + annotated `vX.Y.Z` tag → `otto install`. The `v*` tag
  triggers `.github/workflows/binary-release.yml` to build the x86_64 Linux release tarball.
### 2. Shell functions `clone` / `worktree` → `<bin> shell-init zsh`

The `clone` and `worktree` shell functions are NOT static files and NOT a binary — each
**binary emits its own wrapper function** via a `<bin> shell-init <shell>` subcommand
(`zsh` today; the emitter is structured so `bash`/`fish` can be added later). The function
wraps the binary and `cd`s the parent shell into the printed path. Install is one guarded
`eval` line per tool in your `.zshrc` (matching the house `qai`/`aka` pattern):

```zsh
if hash clone    2>/dev/null; then eval "$(command clone    shell-init zsh)"; fi
if hash worktree 2>/dev/null; then eval "$(command worktree shell-init zsh)"; fi
```

`command <bin>` is load-bearing: it runs the on-PATH binary even if a same-named function
is already defined, so the `eval` always *redefines* the function with the freshly emitted
body (the cutover-ordering fix). The `hash` guard degrades gracefully when the binary isn't
installed. The header comment in the emitted body carries the binary's `GIT_DESCRIBE`, so a
stale function in a long-running shell is diagnosable against `<bin> --version`.

Wrapper contract (do not break it — design: `docs/design/2026-06-29-shell-init-subcommand.md`):

- The **binary** prints the destination path to **stdout**, all errors to **stderr**, and
  exits non-zero on failure.
- The **function** captures stdout, bails on the binary's non-zero exit *before* any `cd`,
  and guards against empty/non-directory output. This is what keeps a failed clone from
  silently `cd`-ing you to `$HOME` (the bug fixed in v0.2.5). If you ever make the binary
  print diagnostics to stdout, you reintroduce that bug.
- `<bin> shell-init` and `-h/--help/-v/--version` pass straight through the function (no
  stdout capture, no `cd`) — so re-emitting interactively after the function is loaded
  prints the script instead of being swallowed.

The function source of truth lives in the binaries: `clone/src/shell.rs` and
`worktree/src/shell.rs` (shared rejection error in `common/src/shell.rs`). The contract is
locked by `tests/shell-functions.zsh` (the `shell-test` otto task), which `eval`s the
*emitted* functions and exercises the prints-path / does-the-`cd` / failed-clone guard.

### 3. Reproducible wiring → the `manifest` CLI

The binary installs (`~/.cargo/bin`) are owned by the `manifest` CLI reading
`scottidler/dotfiles/manifest.yml` — NOT by anything in this repo. The `scottidler/git-tools`
block lists the `cargo:` crates to install. The wrapper functions are NOT a `manifest`
symlink anymore: they come from the two `eval` lines in the dotfiles-tracked `.zshrc`. To
change what gets installed, edit `manifest.yml` and run `manifest`; to change the wrapper
delivery, edit `.zshrc` (the `eval` lines) — there is no longer a `~/.shell-functions.d/*`
link to hand-edit.

### What NOT to do

- **No bash/Python tools here.** git-tools is Rust-only. The Python predecessor `scottidler/git`
  holds the not-yet-ported helpers (`clone-lite`, `default-branch`, `git-objects`,
  `remote-origin-url`, `reponame`); any tool with a Rust twin here was removed from that repo.
  Don't re-add shell/Python reimplementations of workspace crates.
- **Don't reintroduce a static `shell-functions.sh` or a `~/.shell-functions.d/*` symlink.**
  The wrappers are binary-emitted (`<bin> shell-init zsh`) and installed via the `.zshrc`
  `eval` lines; to change the install, edit `dotfiles/manifest.yml` (binaries) or the
  dotfiles `.zshrc` (eval lines) and run `manifest` — never hand-edit live symlinks.
- **Don't `cargo install --path .`** from a single crate dir expecting the whole workspace — use
  `otto install`.
- **Don't print to stdout from the `clone` binary except the destination path** (see wrapper contract).
- **Tagging**: only via `/shipit`/`bump`, only on `main`, annotated, single flat `v*` tag for the
  whole workspace — never per-crate tags.

## Bare-Worktree Layout (`worktree init` opt-in)

`clone org/repo` is acquisition only and always produces a **flat checkout** -
it carries no bare-container code at all. `worktree init org/repo` is the
sole entry point into a **bare container + nested worktrees** layout instead
- an explicit opt-in for the handful of repos where multiple agents work
branches in parallel. `worktree` also owns converting an existing checkout in
place (`migrate`/`flatten`) and the original day-2 lifecycle
(switch/list/prune/pick). Designs: `docs/design/2026-06-21-clone-bare-worktree.md`
(the layout), `docs/design/2026-07-03-clone-flat-default.md` (the flat-default
flip + `--flatten` collapse), `docs/design/2026-07-07-clone-worktree-split.md`
(moving bare/migrate/flatten off `clone` onto `worktree`).

```
~/repos/<org>/<repo>/        # the container (the "logical repo")
  .bare/                     # git clone --bare -> database only, no working files
  .git                       # a FILE, one line: "gitdir: ./.bare" (relative, survives rename)
  <default-branch>/          # the worktree cd lands in (always present)
```

- **Repo-path contract.** `~/repos/<org>/<repo>` is the logical repo (one row in
  discovery); `<repo>/<default-branch>` is the canonical working tree you `cd`
  into. The default-branch worktree is a guaranteed invariant - always present.
- **`.git` pointer is relative** (`gitdir: ./.bare`) so a container can be moved
  with `mv`; per-worktree links are absolute and need `git worktree repair`.
- **Mandatory refspec fix (gotcha).** `git clone --bare` leaves
  `remote.origin.fetch` empty, so remote-tracking branches never populate.
  `worktree init`/`migrate` set `+refs/heads/*:refs/remotes/origin/*` and fetch;
  never skip it.
- **Persona invariant (security).** Worktrees stay under `~/repos/<org>/`, so the
  `~/.gitconfig` `includeIf "gitdir:~/repos/tatari-tv/"` still fires and commits
  carry the work identity. A worktree placed outside the org prefix silently
  reverts to the home identity - never do it. Locked by a unit test
  (`worktree/src/bare/tests.rs::test_persona_invariant_under_org_prefix`).
- **Commands:**
  - `clone org/repo` - flat checkout (the only layout `clone` produces), `cd` into it.
  - `clone --flat org/repo` - redundant no-op alias for the default (`--versioning`
    implies flat). `clone --bare`/`--migrate`/`--flatten` no longer exist - they
    exit non-zero with an unknown-argument error; use the `worktree` verbs below.
  - `worktree init org/repo` - fresh bare container + default worktree, `cd` into it.
    `--clonepath`/`--remote`/`--mirrorpath` override the acquisition inputs
    (defaults: `.` and `REMOTE_URLS[0]`, mirroring `clone`'s own defaults). Run
    against an existing bare container, it reconciles in place; run against an
    existing flat clone, it updates in place and prints a `worktree migrate` hint
    (never clobbers).
  - `worktree <branch>` / `worktree` picker / `worktree --list` / `worktree --prune`
    - unchanged day-2 lifecycle inside an existing bare container.
  - `worktree migrate [org/repo]` - convert a flat checkout to bare. With no
    repospec, migrates the checkout you're standing in (resolves the enclosing
    repo's main worktree, so it works from a subdirectory or a legacy linked
    worktree). Read-only preflight first (requires `rkvr`, resolves the per-org
    SSH key, probes connectivity). Then an additive rescue pass materializes every
    dirty tree (main + linked worktrees), stash, and detached-HEAD worktree as a
    `wip/*` branch - nothing git-tracked is lost. Carries linked worktrees into
    the new container and `rkvr rmrf`s the orphaned external dirs. Preserves
    unpushed commits + local-only branches via the bare-clone-from-local, swaps
    recoverably, repairs worktree links. Bails before mutating on a mid-merge
    tree. Git-ignored files (`.env`) are listed, not carried (recoverable from the
    `rkvr`'d backup); a `target` symlink is noted, not recreated.
  - `worktree flatten [org/repo]` - the reverse: collapse a bare container back to
    a flat checkout. With no repospec, flattens the container you're standing in.
    Refuse-first - it BLOCKS on any unsafe/unmergeable worktree state (uncommitted
    changes, an unmerged/unpushed local branch, a detached HEAD unreachable from a
    ref, an existing stash, an in-progress merge/rebase/cherry-pick/revert/bisect,
    per-worktree config or sparse-checkout, dirty submodules) and on any check that
    cannot be determined (fail-closed). Preserves every `refs/*` at an identical
    OID; archives the whole container via `rkvr` before a copy-then-atomic-swap, so
    a removed worktree's git-ignored files stay recoverable. `worktree migrate|flatten
    --dry-run` previews to stderr with empty stdout, so the shell wrapper never `cd`s.
  - `init`/`migrate`/`flatten` are reserved-word positionals (`argv[1]`),
    dispatched pre-clap in `worktree/src/main.rs` so clap never mistakes them for
    the switch-branch positional; `--list`/`--prune`/other `-*` flags still pass
    straight through with no `cd`.
- **Existing flat clones** are untouched until `migrate`d; `clone org/repo` on
  one updates it in place. `worktree init` on an existing flat clone updates it
  in place and prints a `migrate` hint (mixed ecosystem is supported).
  Discovery (`common::RepoDiscovery`) recognizes both shapes.
- **Config** (`common::config`) is YAML-primary at `~/.config/git-tools/git-tools.yml`
  (`$GIT_TOOLS_CFG` overrides the path), falling back to the legacy INI
  `~/.config/clone/clone.cfg` (`$CLONE_CFG` overrides that path) when the YAML
  file is absent. Carries `default-branch` (fallback default branch) and a
  per-org `orgs.<org>.sshkey` map (`orgs.default` is the catch-all). There is no
  config-driven layout knob anymore - bare is purely explicit via `worktree init`.
  Example: `git-tools.yml.example` at the repo root.
- **No `cd` navigation magic** (the binary-emitted wrappers, see Install & Wiring):
  both wrappers use the
  same contract - the binary prints a destination path to stdout, the shell
  function `cd`s into it. `clone()` does this on a fresh clone; `worktree()` does
  it for `worktree <branch>`/`init`/`migrate`/`flatten`, while the no-arg list form and
  any flag pass straight through (no `cd`). The old `chpwd` shim that redirected
  every `cd`/`z`/pushd into a bare container's default worktree was removed: it
  intercepted all navigation and stranded you on the bare root (relative
  `cd ..`/`cd ../sibling` then resolved against the wrong level). Arriving at a
  bare container now lands you on the container root (standard behavior); worktree
  navigation lives in the separate `worktree` binary, used like `clone` - NOT a
  `git` alias or `chpwd` hook.

Module map: `clone/src/{cli,config,shell}.rs` over a thin `main.rs` + testable
`lib.rs` - flat-only, no `bare`/`migrate`/`flatten`/`transport` module (those
moved to `worktree`/`common`). `worktree/src/{cli,config,bare,init,migrate,
flatten,switch,list,prune,pick,shell}.rs` over `main.rs`/`lib.rs` - owns the
entire bare-container lifecycle. Shared transport (`common/src/transport.rs`)
and config reader (`common/src/config.rs`) live in `common`, consumed by both
binaries; there is no `clone <-> worktree` dependency edge.

## Key Conventions

- **Error handling**: `eyre::Result<T>` everywhere
- **CLI**: `clap` derive macros with `--version` showing `git describe` output
- **Parallelism**: `rayon` for CPU-bound work, `ParallelExecutor` from common
- **Repo discovery**: `RepoDiscovery` scans `max_depth` levels (default 2, `None`
  = unbounded) for `.git` (dir or file) and bare containers (`.bare/`)
- **Slug format**: Always `owner/repo` - derived from git remote URL or filesystem path fallback
- **Git invocation**: route through `common::git::run` (mutations) / `common::git::output`
  (reads); never hand-roll `Command::new("git")` in production code
- **URL parsing**: `common::git::parse_repospec()` is the host-agnostic parser
  (GitHub/GitLab/Bitbucket/enterprise, all URL forms) returning `RepoSpec`;
  `parse_git_url()` is a thin `Option`-returning shim over it
- **Config**: YAML for all configuration (never TOML for config files)
- **Logging**: `--log-level` flag (per crate) wired to `common::log::init`; stderr target, no `RUST_LOG`
- **Tests**: `#[cfg(test)]` modules with `tempfile` for fixtures
- **Versions**: All crates synchronized at same version, released together

## CI

- **Otto** (`.otto.yml`): lint, check (cargo check + clippy + fmt), test, coverage
- **GitHub Actions** (`.github/workflows/binary-release.yml`): triggered on `v*` tags, builds x86_64 Linux binaries

## Common Crate Modules

- `git::parse_repospec(input) -> Result<RepoSpec>` - host-agnostic parser (all URL forms); `git::slugify_branch` lowercase-hyphenates a branch name
- `git::parse_git_url(url) -> Option<String>` - thin shim over `parse_repospec`; `git::get_repo_slug_from_path` reads `origin` from disk
- `git::run` / `git::output` - the single git-command runner (captured stderr, env overrides, `git::ssh_command` for `GIT_SSH_COMMAND`)
- `log::init(level, project)` - `--log-level` -> stderr, idempotent, no `RUST_LOG`
- `repo::RepoInfo` - path + slug (+ opt-in `worktree`) for a repo; bare containers resolve `path` to the default-branch worktree
- `repo::RepoDiscovery` - find repos under paths (`with_max_depth`, `with_per_worktree`), bare-container aware
- `language::detect_language(path) -> Option<String>` - three-stage detection (markers, extensions, fallback)
- `parallel::ParallelExecutor` - rayon-based parallel repo processing
- `bare::is_bare_container(path)` / `bare::default_branch(container, fallback)` - container detection and default-branch resolution; consumed only by `worktree` (`clone` carries no bare-layout code)
- `bare::ref_exists(container, refname) -> bool` - single home for the ref-existence check (used across `worktree`'s `bare`/`migrate`/`flatten`/`prune`; no copies elsewhere)
- `bare::add_worktree(container, &AddSpec) -> Result<PathBuf>` - the guarded `git worktree add` primitive; derives the directory from `slugify_branch(branch)`, applies `Collision::ReuseOrBail` (idempotent re-switch) or `Collision::Uniquify` (batch recreation with numeric suffix)
- `bare::resolve_and_add(container, raw_branch, default_branch) -> Result<PathBuf>` - ref-probing layer used by `worktree`'s switch-or-create (the bare positional `worktree <branch>`): classifies a raw branch string as local / remote-only / new and calls `add_worktree` with `Collision::ReuseOrBail`
- `transport::clone_with_fallback` / `transport::try_clone` / `transport::REMOTE_URLS` - the shared acquisition primitives (SSH-first, HTTPS-fallback `git clone`) used by `clone`'s flat path and `worktree init`/`migrate`
- `config::default_branch() -> Result<Option<String>>` / `config::find_ssh_key_for_org(repospec) -> Result<Option<String>>` - the shared config reader: YAML-primary (`~/.config/git-tools/git-tools.yml`, `$GIT_TOOLS_CFG`), falling back to the legacy INI `clone.cfg` (`$CLONE_CFG`, `~/.config/clone/clone.cfg`) when the YAML file is absent. Fail-closed: the first location whose file exists is THE config for the run - a present-but-malformed file is a loud `Err`, never silently skipped in favor of a lower-precedence file. A missing `default-branch` or an unmatched org is a permissive lookup miss (`None`), not an error.

## Design Docs

Located in `docs/design/`. Created via `/create-design-doc`, executed via `/how-to-execute-a-plan`.
