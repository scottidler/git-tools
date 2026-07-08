// worktree — `init`: fresh bare-container acquisition.
//
// This is the acquisition half of the bare-container lifecycle that used to live
// on `clone --bare`. `worktree init <spec>` clones the remote into a bare
// container (`.bare/` + `.git` pointer + populated `origin/*` + the always-present
// default-branch worktree) and returns the default worktree path for the wrapper
// to `cd` into. It runs from ANY cwd (the container is created under
// `<clonepath>/<org>/<repo>`), so `run` dispatches it without resolving an
// enclosing container.

use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use log::debug;

use crate::bare::{self, AcquireArgs};
use crate::config::Config;

/// Acquire a fresh bare container for `spec`, dispatching on the target's current
/// state (mirroring what `clone --bare` did via `run_bare`):
/// - an existing bare container → idempotent reconcile in place;
/// - an existing *flat* checkout → update it in place and hint at
///   `worktree migrate`, never clobbering;
/// - otherwise → set up a fresh bare container.
///
/// Returns the canonical default-branch worktree path the wrapper `cd`s into.
pub fn init(config: &Config, spec: &RepoSpec) -> Result<PathBuf> {
    let repospec = spec.to_string();
    let target = config.clonepath.join(&repospec);
    debug!("init: repospec={} target={:?}", repospec, target);

    let args = AcquireArgs {
        clonepath: config.clonepath.clone(),
        remote: config.remote.clone(),
        mirrorpath: config.mirrorpath.clone(),
        ssh_key: config.ssh_key.clone(),
        verbose: config.verbose,
        default_branch: config.default_branch.clone(),
    };

    if bare::is_bare_container(&target) {
        debug!("init: '{}' is already a bare container; reconciling", target.display());
        return bare::reconcile_container(&args, &target);
    }

    if is_flat_clone(&target) {
        debug!("init: '{}' is a flat checkout; updating in place", target.display());
        update_flat_in_place(&target)?;
        eprintln!(
            "Note: '{}' is a flat checkout. Run `worktree migrate {}` to convert it to the bare-worktree layout.",
            target.display(),
            repospec
        );
        return Ok(target);
    }

    bare::setup_bare_container(&args, spec)
}

/// Whether `path` is a flat checkout (`.git` dir or file) that is *not* a bare
/// container.
fn is_flat_clone(path: &Path) -> bool {
    let git = path.join(".git");
    (git.is_dir() || git.is_file()) && !bare::is_bare_container(path)
}

/// Update an existing flat checkout in place without clobbering: refuse on
/// untracked files, stash uncommitted changes, then pull. Recovers an empty
/// checkout (a prior failed clone) by fetching and resetting to `origin/HEAD`.
/// Relocated from `clone::lib::update_existing_repo`, checking out `HEAD` (init
/// carries no revision argument).
fn update_flat_in_place(repo: &Path) -> Result<()> {
    debug!("update_flat_in_place: repo={:?}", repo);

    // Handle an empty checkout from a prior failed clone: fetch + reset to remote.
    let head_ok = git::output(&["rev-parse", "HEAD"], Some(repo), None)
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !head_ok {
        if git::run(&["fetch", "origin"], Some(repo), None).is_err() {
            return Err(eyre!(
                "Repository exists but has no commits and fetch failed. Remove {} and try again.",
                repo.display()
            ));
        }
        if git::run(&["reset", "--hard", "origin/HEAD"], Some(repo), None).is_err() {
            return Err(eyre!(
                "Repository exists but failed to reset to remote. Remove {} and try again.",
                repo.display()
            ));
        }
        return Ok(());
    }

    // Never clobber untracked files.
    let status_str = git::output(&["status", "--porcelain"], Some(repo), None)
        .wrap_err("Failed to check git status")?
        .stdout;
    let has_untracked = status_str.lines().any(|line| line.starts_with("??"));
    if has_untracked {
        return Err(eyre!(
            "Cannot update repository: untracked files present.\n\
             Please commit, remove, or add them to .gitignore before running init.\n\
             Untracked files:\n{}",
            status_str
                .lines()
                .filter(|line| line.starts_with("??"))
                .map(|line| line.trim_start_matches("?? "))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    // Stash any uncommitted (tracked) changes so the pull is clean.
    if !status_str.is_empty() {
        git::run(
            &["stash", "push", "-m", "Automatic stash by worktree init"],
            Some(repo),
            None,
        )?;
        eprintln!("Note: Uncommitted changes have been stashed. Use 'git stash pop' to restore them.");
    }

    git::run(&["pull"], Some(repo), None)?;
    Ok(())
}

#[cfg(test)]
mod tests;
