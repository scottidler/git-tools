// worktree — list the worktrees in a bare container.

use std::path::{Path, PathBuf};

use common::bare;
use eyre::Result;
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

impl From<bare::WorktreeRow> for Entry {
    fn from(row: bare::WorktreeRow) -> Self {
        Entry {
            path: row.path,
            branch: row.branch,
            bare: row.bare,
            locked: row.locked,
        }
    }
}

/// List `container`'s worktrees via the shared `common::bare::resolve_worktrees`
/// parser.
pub fn list(container: &Path) -> Result<Vec<Entry>> {
    debug!("list: container={:?}", container);
    let entries = bare::resolve_worktrees(container)?
        .into_iter()
        .map(Entry::from)
        .collect();
    Ok(entries)
}

#[cfg(test)]
mod tests;
