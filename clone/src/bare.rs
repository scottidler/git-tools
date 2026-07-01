// clone — bare-container + nested-worktree setup.
//
// A bare container is `~/repos/<org>/<repo>/` holding `.bare/` (the git
// database, no working files), a `.git` pointer file (`gitdir: ./.bare`), and
// one worktree directory per checked-out branch. The default-branch worktree is
// a guaranteed invariant: it is always present so "the repo" resolves to a real
// working tree, and the wrapper `cd`s the user into it.

use std::fs;
use std::path::{Path, PathBuf};

use common::bare::{AddSpec, Collision, Source};
use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, bail};
use log::{debug, warn};

use crate::config::Config;
use crate::transport;

// Bare-container primitives shared with `worktree` live in `common::bare`; a
// bare container also carries a `.git` pointer file, so callers must check
// `is_bare_container` before treating a `.git` as a flat checkout.
pub use common::bare::{default_branch, is_bare_container, ref_exists};

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
    // Now that origin/* exists, link each local head to its upstream. A
    // `git clone --bare` writes refs/heads/* directly but records no tracking
    // config, so without this `git pull` in a worktree fails with "no tracking
    // information for the current branch".
    link_upstreams(container)?;
    Ok(())
}

/// Set `branch.<name>.remote`/`.merge` for every local head that has a matching
/// `origin/<name>` ref but no upstream yet. A `git clone --bare` populates
/// `refs/heads/*` from the remote without recording tracking config (a non-bare
/// clone does this for the checked-out branch), and `git worktree add <dir>
/// <branch>` checks out an existing local branch as-is without adding tracking -
/// so the default worktree's `main` ends up with no upstream and `git pull`
/// fails. Idempotent: a branch that already has an upstream is left untouched,
/// so a reconcile re-run never clobbers a deliberate re-point. A per-branch
/// failure is logged and skipped rather than aborting an otherwise-good clone.
fn link_upstreams(container: &Path) -> Result<()> {
    debug!("link_upstreams: container={:?}", container);
    let heads = git::output(
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        Some(container),
        None,
    )?;
    if !heads.status.success() {
        bail!(
            "git for-each-ref refs/heads failed in {}: {}",
            container.display(),
            heads.stdout.trim()
        );
    }

    for branch in heads.stdout.lines().map(str::trim).filter(|b| !b.is_empty()) {
        if branch_has_upstream(container, branch) {
            debug!("link_upstreams: '{}' already has an upstream; skipping", branch);
            continue;
        }
        if !ref_exists(container, &format!("refs/remotes/origin/{}", branch)) {
            debug!(
                "link_upstreams: no origin/{} ref; leaving '{}' untracked",
                branch, branch
            );
            continue;
        }
        let upstream = format!("origin/{}", branch);
        match git::output(
            &["branch", "--set-upstream-to", &upstream, branch],
            Some(container),
            None,
        ) {
            Ok(out) if out.status.success() => {
                debug!("link_upstreams: set upstream {} for '{}'", upstream, branch);
            }
            Ok(out) => {
                warn!(
                    "link_upstreams: could not set upstream {} for '{}': {}",
                    upstream,
                    branch,
                    out.stderr.trim()
                );
            }
            Err(e) => {
                warn!(
                    "link_upstreams: setting upstream {} for '{}' failed: {}",
                    upstream, branch, e
                );
            }
        }
    }
    Ok(())
}

/// Whether `branch` already has an upstream recorded (`branch.<name>.merge` set).
fn branch_has_upstream(container: &Path, branch: &str) -> bool {
    git::output(
        &["config", "--get", &format!("branch.{}.merge", branch)],
        Some(container),
        None,
    )
    .map(|o| o.status.success() && !o.stdout.trim().is_empty())
    .unwrap_or(false)
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
    // Flow through the shared primitive with `ReuseOrBail`: it locates an
    // already-checked-out default by BRANCH (via `git worktree list`), so a
    // re-run is idempotent and a legacy container whose worktree sits at the
    // pre-slug raw path is reused rather than double-checked-out. We must NOT
    // pre-check `container.join(branch)` here: that would bypass the primitive's
    // by-branch reuse and reintroduce the raw-path/slug-path split.
    add_worktree(container, &branch)
}

/// Whether the container's `HEAD` resolves to a commit (false for an empty
/// remote with no branches).
fn has_commits(container: &Path) -> bool {
    git::output(&["rev-parse", "--verify", "--quiet", "HEAD"], Some(container), None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Add a worktree for an existing local branch via the shared `common::bare`
/// primitive. The directory is derived as `slugify_branch(branch)` (so a slashed
/// default lands at a safe slug dir, matching the `worktree` tool), and
/// `ReuseOrBail` makes the add idempotent / legacy-raw-path compatible by
/// locating the branch via `git worktree list` rather than the derived dir.
/// Returns the worktree path.
pub fn add_worktree(container: &Path, branch: &str) -> Result<PathBuf> {
    debug!("add_worktree: container={:?} branch={}", container, branch);
    common::bare::add_worktree(
        container,
        &AddSpec {
            branch,
            source: Source::ExistingLocal,
            collision: Collision::ReuseOrBail,
        },
    )
}

#[cfg(test)]
mod tests;
