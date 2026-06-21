# Agents

Guidelines for AI agents working in this repository.

## Before You Start

- Read `CLAUDE.md` for workspace structure and conventions
- Check `docs/design/` for any relevant design docs
- Run `otto ci` before committing to validate changes

## This is a Rust Workspace

All tools are Rust CLI binaries sharing a common library. When making changes:

- Changes to `common/` affect all tools - run full workspace tests
- Each binary has its own `src/main.rs` - keep them thin
- Shared logic belongs in `common/`, not duplicated across binaries

## Git Remote URL Formats

The host-agnostic parser is `common::git::parse_repospec` (in
`common/src/git/spec.rs`), returning `RepoSpec { org, repo }`. It handles
`org/repo`, `https://`, `http://`, `git@host:org/repo` (SCP-style), `ssh://`,
and `git://`, with `.git` stripping and extra-path-segment tolerance, for any
host (GitHub/GitLab/Bitbucket/enterprise). `common::git::parse_git_url` is a thin
`Option`-returning shim over it. If you add a new URL format, add tests in
`common/src/git/spec.rs`.

When URL parsing fails, `slug_from_path()` derives org/repo from the filesystem path. Never fall back to hardcoded `"unknown/unknown"`.

All git subprocesses route through `common::git::run` / `common::git::output`;
do not hand-roll `Command::new("git")` in production code.

## Repo Discovery

`RepoDiscovery` scans `max_depth` levels (default 2, handles `~/repos/org/repo`;
`with_max_depth(None)` is unbounded). It:
- Recognizes a `.git` directory or file, and bare containers (`.bare/`)
- Stops descending once a path is recognized as a repo or bare container
- Emits one `RepoInfo` per logical repo (bare container `path` = its
  default-branch worktree); per-worktree rows are opt-in via `with_per_worktree`

## Testing

- Unit tests go in `#[cfg(test)] mod tests` blocks
- Use `tempfile::TempDir` for filesystem fixtures
- Always test both success and error paths
- Run `cargo test --workspace` to catch cross-crate breakage

## Commit and Release

- Commit messages: conventional commits (`feat`, `fix`, `refactor`, `docs`, `chore`)
- All crates share a single version - bump via `/bump`
- Never delete git tags
- Push triggers CI; tags matching `v*` trigger binary releases
