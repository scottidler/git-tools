// worktree — switch to (or create) the worktree for a branch.
//
// Branch-source selection uses the **raw** argument first so existing non-slug
// branches stay reachable; only a brand-new branch is slugified. New worktrees
// of remote branches are created `--track`ing, so upstream is set from the
// start (the bare-clone upstream gotcha at the worktree level).

use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, WrapErr, bail};
use log::debug;

use crate::bare;

/// Switch to (or create) a worktree for `raw_branch` in `container`, returning
/// the worktree path the wrapper `cd`s into.
pub fn switch(container: &Path, raw_branch: &str, default_branch: Option<&str>) -> Result<PathBuf> {
    debug!(
        "switch: container={:?} raw_branch={} default_branch={:?}",
        container, raw_branch, default_branch
    );

    // 1. Existing local branch → check it out as-is into a slugified dir.
    if ref_exists(container, &format!("refs/heads/{}", raw_branch)) {
        let dir = git::slugify_branch(raw_branch);
        return ensure_or_add(container, &dir, raw_branch, &["worktree", "add", &dir, raw_branch]);
    }

    // 2. Existing remote branch → create a tracking local branch (real name),
    //    slugified dir.
    if ref_exists(container, &format!("refs/remotes/origin/{}", raw_branch)) {
        let dir = git::slugify_branch(raw_branch);
        let origin_ref = format!("origin/{}", raw_branch);
        return ensure_or_add(
            container,
            &dir,
            raw_branch,
            &["worktree", "add", "-b", raw_branch, "--track", &dir, &origin_ref],
        );
    }

    // 3. New branch → slugify, use the slug as both branch and dir, based on the
    //    default branch.
    let slug = git::slugify_branch(raw_branch);
    if slug.is_empty() {
        bail!(
            "branch name '{}' slugifies to empty; choose a name with alphanumerics",
            raw_branch
        );
    }
    let base = bare::default_branch(container, default_branch)?;
    ensure_or_add(container, &slug, &slug, &["worktree", "add", "-b", &slug, &slug, &base])
}

/// Add the worktree unless its directory already exists (idempotent: a re-run
/// just `cd`s into the existing worktree). `branch` is the branch the worktree
/// is meant to host; an existing dir is only reused when it actually does.
/// Returns the worktree path.
fn ensure_or_add(container: &Path, dir: &str, branch: &str, add_args: &[&str]) -> Result<PathBuf> {
    let worktree = container.join(dir);
    if worktree.exists() {
        // A linked worktree carries a `.git` FILE (the gitdir pointer). Reuse a
        // real worktree; refuse to `cd` into an unrelated directory that merely
        // shares the name.
        if worktree.join(".git").is_file() {
            // `slugify_branch` collapses '/', spaces, dots and hyphens into one
            // namespace, so distinct branches (e.g. `feature/auth` and a literal
            // `feature-auth`) can map to the same dir. Reuse only when the dir
            // actually hosts the intended branch; never silently `cd` into a
            // colliding tree.
            let head = git::output(&["symbolic-ref", "--short", "HEAD"], Some(&worktree), None)?;
            let current = head.stdout.trim();
            if head.status.success() && current == branch {
                debug!(
                    "ensure_or_add: '{}' already hosts '{}'; reusing",
                    worktree.display(),
                    branch
                );
                return Ok(worktree);
            }
            bail!(
                "'{}' already hosts branch '{}', not '{}' (slug collision); refusing to reuse it",
                worktree.display(),
                if current.is_empty() { "(detached)" } else { current },
                branch
            );
        }
        bail!(
            "'{}' exists but is not a git worktree; refusing to reuse it",
            worktree.display()
        );
    }
    git::run(add_args, Some(container), None)
        .wrap_err_with(|| format!("git {:?} in {}", add_args, container.display()))?;
    Ok(worktree)
}

/// Whether `refname` resolves in the container's git database.
fn ref_exists(container: &Path, refname: &str) -> bool {
    git::output(&["rev-parse", "--verify", "--quiet", refname], Some(container), None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
