// worktree — list the worktrees in a bare container.

use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, bail};
use log::debug;

/// One entry from `git worktree list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// The checked-out branch, or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The bare repository itself (no working tree to `cd` into).
    pub bare: bool,
    /// A locked worktree (`git worktree lock`); prune must skip it.
    pub locked: bool,
}

/// Parse `git worktree list --porcelain` for `container`.
pub fn list(container: &Path) -> Result<Vec<Entry>> {
    debug!("list: container={:?}", container);
    let out = git::output(&["worktree", "list", "--porcelain"], Some(container), None)?;
    if !out.status.success() {
        bail!(
            "git worktree list failed in {}: {}",
            container.display(),
            out.stderr.trim()
        );
    }
    Ok(parse(&out.stdout))
}

/// Parse the porcelain stream into entries. Blocks are separated by blank lines;
/// each opens with `worktree <path>`, then optional `bare` / `branch <ref>` /
/// `detached` / `HEAD <sha>` lines.
fn parse(porcelain: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    let mut cur: Option<Entry> = None;

    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(entry) = cur.take() {
                entries.push(entry);
            }
            cur = Some(Entry {
                path: PathBuf::from(rest),
                branch: None,
                bare: false,
                locked: false,
            });
        } else if line == "bare"
            && let Some(entry) = cur.as_mut()
        {
            entry.bare = true;
        } else if let Some(refname) = line.strip_prefix("branch ")
            && let Some(entry) = cur.as_mut()
        {
            entry.branch = Some(refname.trim_start_matches("refs/heads/").to_string());
        } else if (line == "locked" || line.starts_with("locked "))
            && let Some(entry) = cur.as_mut()
        {
            // porcelain emits bare `locked` or `locked <reason>`.
            entry.locked = true;
        }
        // `HEAD <sha>` and `detached` leave `branch` as None; blank lines end a block.
    }
    if let Some(entry) = cur.take() {
        entries.push(entry);
    }
    entries
}

#[cfg(test)]
mod tests;
