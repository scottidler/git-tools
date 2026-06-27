// clone — bare-container + nested-worktree setup.
//
// A bare container is `~/repos/<org>/<repo>/` holding `.bare/` (the git
// database, no working files), a `.git` pointer file (`gitdir: ./.bare`), and
// one worktree directory per checked-out branch. The default-branch worktree is
// a guaranteed invariant: it is always present so "the repo" resolves to a real
// working tree, and the wrapper `cd`s the user into it.

use std::fs;
use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use log::{debug, warn};

use crate::config::Config;
use crate::transport;

/// Whether `path` is a bare container (`.bare/` present). A bare container also
/// carries a `.git` pointer file, so callers must check this before treating a
/// `.git` as a flat checkout.
pub fn is_bare_container(path: &Path) -> bool {
    path.join(".bare").is_dir()
}

/// Set up a fresh bare container for `spec`, returning the canonical
/// default-branch worktree path the wrapper `cd`s into (or the container itself
/// for a commitless remote).
pub fn setup_bare_container(config: &Config, spec: &RepoSpec) -> Result<PathBuf> {
    let repospec = spec.to_string();
    let container = config.clonepath.join(&repospec);
    let bare = container.join(".bare");
    debug!("setup_bare_container: repospec={} container={:?}", repospec, container);

    fs::create_dir_all(&container).wrap_err_with(|| format!("creating container {:?}", container))?;

    // 1. bare clone into .bare (SSH key + SSH->HTTPS fallback, as for flat).
    //    On failure, remove the container we just created so a failed clone
    //    never leaves an empty turd directory behind (matches the flat path).
    if let Err(e) = transport::clone_with_fallback(
        &repospec,
        &bare,
        &config.remote,
        config.mirrorpath.as_deref(),
        config.ssh_key.as_deref(),
        &["--bare"],
        config.verbose,
    ) {
        let _ = fs::remove_dir_all(&container);
        return Err(e);
    }

    // 2. write the `.git` pointer so the container resolves to `.bare`.
    write_git_pointer(&container)?;

    // 3. mandatory refspec fix: a --bare clone leaves remote.origin.fetch empty.
    fix_fetch_refspec(&container, None)?;

    // 4 + 5. materialize the always-present default-branch worktree.
    ensure_default_worktree(config, &container)
}

/// Reconcile an existing bare container (idempotent re-run): re-apply the
/// refspec fix, fetch, and ensure the default worktree exists. Returns the
/// default worktree path.
pub fn reconcile_container(config: &Config, container: &Path) -> Result<PathBuf> {
    debug!("reconcile_container: container={:?}", container);
    // The pointer should exist, but a half-finished prior run might have left it
    // out; writing it is cheap and idempotent.
    write_git_pointer(container)?;
    fix_fetch_refspec(container, None)?;
    ensure_default_worktree(config, container)
}

/// Write `<container>/.git` containing the bare pointer line. The pointer is
/// relative (`gitdir: ./.bare`), so it survives a container rename.
pub(crate) fn write_git_pointer(container: &Path) -> Result<()> {
    let pointer = container.join(".git");
    fs::write(&pointer, "gitdir: ./.bare\n").wrap_err_with(|| format!("writing {:?}", pointer))?;
    Ok(())
}

/// Repair the empty fetch refspec a `git clone --bare` leaves behind and
/// populate remote-tracking branches. Without this, `git branch -r` is empty
/// and worktrees cannot track `origin/*`. `envs` carries network overrides such
/// as a per-org `GIT_SSH_COMMAND` (the normal clone path passes `None`).
pub fn fix_fetch_refspec(container: &Path, envs: Option<&[(&str, &str)]>) -> Result<()> {
    debug!("fix_fetch_refspec: container={:?}", container);
    git::run(
        &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"],
        Some(container),
        None,
    )?;
    git::run(&["fetch", "origin"], Some(container), envs)?;
    Ok(())
}

/// Determine the container's default branch. A fresh `git clone --bare` sets
/// `HEAD` to the remote default, so `symbolic-ref --short HEAD` yields it; fall
/// back to `origin/HEAD`, then to `git remote set-head origin -a`, then to the
/// `clone.cfg` `[clone] default`. Never hardcodes `main`.
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
        warn!("default_branch: falling back to clone.cfg default '{}'", branch);
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

/// Ensure the default-branch worktree exists, returning its path. For a
/// commitless remote (freshly created repo), skip the worktree add and return
/// the container path so the wrapper still lands the user somewhere real.
fn ensure_default_worktree(config: &Config, container: &Path) -> Result<PathBuf> {
    if !has_commits(container) {
        warn!(
            "ensure_default_worktree: '{}' has no commits (empty remote); skipping worktree add",
            container.display()
        );
        return Ok(container.to_path_buf());
    }

    let branch = default_branch(container, config.default_branch.as_deref())?;
    let worktree = container.join(&branch);
    if worktree.is_dir() {
        debug!("ensure_default_worktree: '{}' already present", worktree.display());
        return Ok(worktree);
    }
    add_worktree(container, &branch)
}

/// Whether the container's `HEAD` resolves to a commit (false for an empty
/// remote with no branches).
fn has_commits(container: &Path) -> bool {
    git::output(&["rev-parse", "--verify", "--quiet", "HEAD"], Some(container), None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Add a worktree for an existing local branch, checking it out into a
/// directory of the same name under the container. Returns the worktree path.
pub fn add_worktree(container: &Path, branch: &str) -> Result<PathBuf> {
    debug!("add_worktree: container={:?} branch={}", container, branch);
    let worktree = container.join(branch);
    git::run(&["worktree", "add", branch, branch], Some(container), None)
        .wrap_err_with(|| format!("git worktree add {} in {:?}", branch, container.display()))?;
    Ok(worktree)
}

#[cfg(test)]
mod tests;
