// worktree — bare-container helpers.
//
// The shared primitives (`is_bare_container`, `default_branch`) live in
// `common::bare` so `clone` and `worktree` can't drift. This module adds the
// worktree-tool-specific container resolution.

use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, WrapErr, bail, eyre};
use log::debug;

pub use common::bare::{default_branch, is_bare_container};

/// Resolve the bare container enclosing the current directory.
///
/// `git rev-parse --git-common-dir` returns `<container>/.bare` from anywhere
/// inside any of its worktrees; the container is that directory's parent.
pub fn resolve_container_from_cwd() -> Result<PathBuf> {
    debug!("resolve_container_from_cwd");
    let out = git::output(&["rev-parse", "--git-common-dir"], None, None)?;
    if !out.status.success() {
        bail!("not inside a git repository; run worktree inside a bare container");
    }

    // `--git-common-dir` may be relative to CWD; canonicalize, then take the
    // parent of `.bare`.
    let common_dir = std::fs::canonicalize(out.stdout.trim())
        .wrap_err_with(|| format!("resolving git common dir '{}'", out.stdout.trim()))?;
    common_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| eyre!("git common dir '{}' has no parent", common_dir.display()))
}
