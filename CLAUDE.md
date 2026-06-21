# git-tools

A Rust workspace of CLI tools for git repository discovery, analysis, and management.

## Workspace Structure

- **`common/`** - Shared library with git URL parsing, repo discovery, language detection, and parallel execution
- **`clone/`** - Clone repos from various spec formats (org/repo, SSH, HTTPS)
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
- `~/bin/clone` is a symlink to `~/.cargo/bin/clone`; the shell function below calls `~/bin/clone`.

### 2. Shell function `clone` → `shell-functions.sh`

`shell-functions.sh` (repo root) defines the `clone` shell function. It is NOT a binary — it
wraps the `clone` binary and `cd`s into the freshly cloned path. Wrapper contract (do not break it):

- The `clone` **binary** prints the destination path to **stdout**, all errors to **stderr**,
  and exits non-zero on failure.
- The **function** captures stdout, bails on the binary's non-zero exit *before* any `cd`, and
  guards against empty/non-directory output. This is what keeps a failed clone from silently
  `cd`-ing you to `$HOME` (the bug fixed in v0.2.5). If you ever make the binary print
  diagnostics to stdout, you reintroduce that bug.

Live wiring: `~/.shell-functions.d/git-tools.sh` → this repo's `shell-functions.sh`, sourced by
`~/.shell-functions` on shell startup.

### 3. Reproducible wiring → the `manifest` CLI

The symlinks (`~/.cargo/bin` installs and `~/.shell-functions.d/*` links) are owned by the
`manifest` CLI reading `scottidler/dotfiles/manifest.yml` — NOT by anything in this repo. The
`scottidler/git-tools` block lists the `cargo:` crates to install and the `link:` entries
(e.g. `shell-functions.sh: ~/.shell-functions.d/git-tools.sh`). To change what gets installed or
linked, edit `manifest.yml` and run `manifest`; do not hand-edit the symlinks.

### What NOT to do

- **No bash/Python tools here.** git-tools is Rust-only. The Python predecessor `scottidler/git`
  holds the not-yet-ported helpers (`clone-lite`, `default-branch`, `git-objects`,
  `remote-origin-url`, `reponame`); any tool with a Rust twin here was removed from that repo.
  Don't re-add shell/Python reimplementations of workspace crates.
- **Don't hand-edit `~/.shell-functions.d/*` or `~/bin/*` symlinks** as the fix — change
  `dotfiles/manifest.yml` so the state is reproducible, then run `manifest`.
- **Don't `cargo install --path .`** from a single crate dir expecting the whole workspace — use
  `otto install`.
- **Don't print to stdout from the `clone` binary except the destination path** (see wrapper contract).
- **Tagging**: only via `/shipit`/`bump`, only on `main`, annotated, single flat `v*` tag for the
  whole workspace — never per-crate tags.

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

## Design Docs

Located in `docs/design/`. Created via `/create-design-doc`, executed via `/how-to-execute-a-plan`.
