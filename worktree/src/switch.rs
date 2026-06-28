// worktree — switch to (or create) the worktree for a branch.
//
// The ref-probing 3-case logic (local / remote-only / new) lives in
// `common::bare::resolve_and_add`; this module is a thin delegation layer
// so the `worktree` tool's `Op::Switch` arm has a named call site.

use std::path::{Path, PathBuf};

use common::bare;
use eyre::Result;
use log::debug;

/// Switch to (or create) a worktree for `raw_branch` in `container`, returning
/// the worktree path the wrapper `cd`s into.
pub fn switch(container: &Path, raw_branch: &str, default_branch: Option<&str>) -> Result<PathBuf> {
    debug!(
        "switch: container={:?} raw_branch={} default_branch={:?}",
        container, raw_branch, default_branch
    );
    bare::resolve_and_add(container, raw_branch, default_branch)
}

#[cfg(test)]
mod tests;
