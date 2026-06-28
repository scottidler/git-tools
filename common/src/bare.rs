// common — bare-container primitives shared by `clone` and `worktree`.
//
// A bare container is `~/repos/<org>/<repo>/` holding `.bare/` (the git
// database), a `.git` pointer file, and one worktree directory per checked-out
// branch. These read-and-occasionally-mutate primitives are shared so the two
// binaries can't drift (notably `default_branch`, which mutates via
// `git remote set-head`).

use std::path::Path;

use eyre::{Result, eyre};
use log::{debug, warn};

use crate::git;

/// Whether `path` is a bare container (`.bare/` present).
pub fn is_bare_container(path: &Path) -> bool {
    path.join(".bare").is_dir()
}

/// Determine the container's default branch. A `git clone --bare` sets `HEAD` to
/// the remote default, so `symbolic-ref --short HEAD` yields it; fall back to
/// `origin/HEAD`, then to `git remote set-head origin -a`, then to `fallback`.
/// Never hardcodes `main`.
pub fn default_branch(container: &Path, fallback: Option<&str>) -> Result<String> {
    debug!("default_branch: container={:?} fallback={:?}", container, fallback);

    if let Some(branch) = symbolic_ref_short(container, "HEAD") {
        return Ok(branch);
    }
    if let Some(branch) = symbolic_ref_short(container, "refs/remotes/origin/HEAD") {
        return Ok(branch);
    }

    // origin/HEAD may be unset; ask the remote to populate it, then retry.
    let _ = git::run(&["remote", "set-head", "origin", "-a"], Some(container), None);
    if let Some(branch) = symbolic_ref_short(container, "refs/remotes/origin/HEAD") {
        return Ok(branch);
    }

    if let Some(branch) = fallback {
        warn!("default_branch: falling back to '{}'", branch);
        return Ok(branch.to_string());
    }

    Err(eyre!(
        "could not determine default branch for bare container '{}'",
        container.display()
    ))
}

/// `git symbolic-ref --short <refname>`, returning the branch name with any
/// `origin/` prefix stripped, or `None` when the ref is unset/missing.
fn symbolic_ref_short(container: &Path, refname: &str) -> Option<String> {
    let out = git::output(&["symbolic-ref", "--short", refname], Some(container), None).ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = out.stdout.trim().trim_start_matches("origin/").to_string();
    if branch.is_empty() { None } else { Some(branch) }
}
