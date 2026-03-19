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

The `common::git::parse_git_url` function handles three GitHub URL formats:
- `git@github.com:owner/repo.git` (SSH shorthand)
- `https://github.com/owner/repo.git` (HTTPS)
- `ssh://git@github.com/owner/repo.git` (SSH protocol)

If you add a new URL format, add tests in `common/src/git/url_parser.rs`.

When URL parsing fails, `slug_from_path()` derives org/repo from the filesystem path. Never fall back to hardcoded `"unknown/unknown"`.

## Repo Discovery

`RepoDiscovery` scans up to 2 levels deep (handles `~/repos/org/repo` layout). It:
- Checks if a path itself is a git repo
- Scans first-level children
- Scans second-level children (for org/ directories)

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
