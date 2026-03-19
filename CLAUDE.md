# git-tools

A Rust workspace of CLI tools for git repository discovery, analysis, and management.

## Workspace Structure

- **`common/`** - Shared library with git URL parsing, repo discovery, language detection, and parallel execution
- **`clone/`** - Clone repos from various spec formats (org/repo, SSH, HTTPS)
- **`filter-ref/`** - Analyze git refs with age/author filtering
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

## Key Conventions

- **Error handling**: `eyre::Result<T>` everywhere
- **CLI**: `clap` derive macros with `--version` showing `git describe` output
- **Parallelism**: `rayon` for CPU-bound work, `ParallelExecutor` from common
- **Repo discovery**: `RepoDiscovery` scans up to 2 levels deep for `.git/` dirs
- **Slug format**: Always `owner/repo` - derived from git remote URL or filesystem path fallback
- **URL parsing**: `parse_git_url()` handles `git@github.com:`, `https://github.com/`, and `ssh://git@github.com/`
- **Config**: YAML for all configuration (never TOML for config files)
- **Logging**: `env_logger` + `log` macros
- **Tests**: `#[cfg(test)]` modules with `tempfile` for fixtures
- **Versions**: All crates synchronized at same version, released together

## CI

- **Otto** (`.otto.yml`): lint, check (cargo check + clippy + fmt), test, coverage
- **GitHub Actions** (`.github/workflows/binary-release.yml`): triggered on `v*` tags, builds x86_64 Linux binaries

## Common Crate Modules

- `git::parse_git_url(url) -> Option<String>` - URL to owner/repo
- `repo::RepoInfo` - path + slug for a repo
- `repo::RepoDiscovery` - find repos under paths with smart matching
- `language::detect_language(path) -> Option<String>` - three-stage detection (markers, extensions, fallback)
- `parallel::ParallelExecutor` - rayon-based parallel repo processing

## Design Docs

Located in `docs/design/`. Created via `/create-design-doc`, executed via `/how-to-execute-a-plan`.
