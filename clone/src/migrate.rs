// clone — `--migrate`: convert a flat checkout into a bare container without
// losing local work.
//
// The original tree is never mutated until a verified, recoverable swap: the
// bare container is built from the LOCAL repo (preserving unpushed commits and
// local-only branches), staged alongside as `<repo>.migrating`, verified, then
// rename-swapped (`<repo>` → `<repo>.backup` → `<repo>.migrating` → `<repo>`),
// re-verified, and only then is the backup removed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::git;
use eyre::{Result, WrapErr, bail, eyre};
use log::{debug, warn};

use crate::bare;

/// Convert the flat checkout at `flat` into a bare container in place,
/// returning the canonical default-branch worktree path. `default_fallback` is
/// the `clone.cfg` `[clone] default` used only if the remote advertises no
/// default branch.
pub fn migrate_flat_to_bare(flat: &Path, default_fallback: Option<&str>) -> Result<PathBuf> {
    debug!("migrate_flat_to_bare: flat={:?}", flat);

    if !flat.is_dir() || !flat.join(".git").exists() {
        bail!("'{}' is not a git checkout to migrate", flat.display());
    }
    if bare::is_bare_container(flat) {
        bail!("'{}' is already a bare container", flat.display());
    }

    // 1. Refuse a dirty or stashed tree; never auto-resolve, never lose work.
    ensure_clean(flat)?;

    // 2. Capture the real origin URL and the currently checked-out branch.
    let origin_url = origin_url(flat)?;
    let current = current_branch(flat);
    warn_dropped_state(flat);

    // 3. Clone the bare container from the LOCAL repo (captures every local ref
    //    at its local state - unpushed commits and local-only branches).
    let migrating = sibling(flat, "migrating")?;
    remove_dir(&migrating)?; // clear any leftover from a failed prior run
    let bare = migrating.join(".bare");
    fs::create_dir_all(&migrating).wrap_err_with(|| format!("creating {}", migrating.display()))?;

    if let Err(e) = git::run(
        &["clone", "--bare", &flat.to_string_lossy(), &bare.to_string_lossy()],
        None,
        None,
    ) {
        let _ = remove_dir(&migrating);
        return Err(e).wrap_err("bare-clone-from-local failed");
    }

    // 4. Repoint at the real remote, then refspec fix + fetch (updates
    //    refs/remotes/origin/* only, preserving the local-ahead refs/heads/*).
    git::run(&["remote", "set-url", "origin", &origin_url], Some(&bare), None)?;
    bare::write_git_pointer(&migrating)?;
    bare::fix_fetch_refspec(&migrating)?;

    // 5. Determine the TRUE default branch from the REMOTE. The bare clone's
    //    HEAD reflects the flat repo's checked-out branch (which may not be the
    //    default), so HEAD-first detection would pick the wrong branch. Create
    //    the always-present default-branch worktree, reset the container HEAD to
    //    it (so the cd/z shim, discovery, and reconcile all agree on the
    //    canonical worktree), then add the previously checked-out branch too
    //    when it differs.
    let default = origin_default_branch(&migrating, default_fallback)?;
    let mut worktrees = vec![add_default_worktree(&migrating, &default)?];
    git::run(
        &["symbolic-ref", "HEAD", &format!("refs/heads/{}", default)],
        Some(&migrating),
        None,
    )
    .wrap_err("resetting container HEAD to the default branch")?;
    if let Some(cur) = current.as_deref()
        && cur != default
        && cur != "HEAD"
        && !cur.is_empty()
    {
        let dir = git::slugify_branch(cur);
        worktrees.push(add_named_worktree(&migrating, &dir, cur)?);
    }

    // 6. Verify the staged container, then perform the recoverable swap.
    verify(&migrating.join(&default), &origin_url)?;

    let backup = sibling(flat, "backup")?;
    remove_dir(&backup)?;
    fs::rename(flat, &backup).wrap_err_with(|| format!("renaming {} aside", flat.display()))?;
    if let Err(e) = fs::rename(&migrating, flat) {
        // Swap-in failed: restore the original from backup.
        let _ = fs::rename(&backup, flat);
        return Err(e).wrap_err("swapping the migrated container into place failed");
    }

    // Worktree admin files store absolute paths recorded at the staging path;
    // repair them to the final location, then re-verify. A failure in EITHER
    // step rolls back to the original (backup) so a broken container never
    // replaces a healthy checkout.
    let final_worktree = flat.join(&default);
    if let Err(e) = repair_worktrees(flat, &worktrees).and_then(|()| verify(&final_worktree, &origin_url)) {
        let _ = remove_dir(flat);
        let _ = fs::rename(&backup, flat);
        return Err(e).wrap_err("migrated container failed repair/verification after swap");
    }

    // 7. Migration committed; remove the backup (best-effort - the repo is
    //    already live and correct).
    if let Err(e) = remove_dir(&backup) {
        warn!("migrate: could not remove backup {}: {}", backup.display(), e);
    }

    Ok(final_worktree)
}

/// Refuse to migrate a dirty (uncommitted/untracked) or stashed tree.
fn ensure_clean(flat: &Path) -> Result<()> {
    let status = git::output(&["status", "--porcelain"], Some(flat), None)?;
    if !status.stdout.trim().is_empty() {
        bail!(
            "refusing to migrate '{}': working tree has uncommitted or untracked changes.\n\
             Commit them, or add untracked files to .gitignore, then re-run --migrate.",
            flat.display()
        );
    }

    let stash = git::output(&["stash", "list"], Some(flat), None)?;
    if !stash.stdout.trim().is_empty() {
        bail!(
            "refusing to migrate '{}': the stash is non-empty.\n\
             Resolve it (e.g. `git stash branch <tmp>`) then re-run --migrate.",
            flat.display()
        );
    }

    Ok(())
}

/// The `origin` remote URL of the flat checkout.
fn origin_url(flat: &Path) -> Result<String> {
    let out = git::output(&["remote", "get-url", "origin"], Some(flat), None)?;
    if !out.status.success() {
        bail!("'{}' has no 'origin' remote to migrate", flat.display());
    }
    Ok(out.stdout.trim().to_string())
}

/// The currently checked-out branch, or `None` when detached.
fn current_branch(flat: &Path) -> Option<String> {
    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(flat), None).ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = out.stdout.trim().to_string();
    if branch.is_empty() || branch == "HEAD" { None } else { Some(branch) }
}

/// Warn about machine-local state that does NOT travel with a bare clone.
fn warn_dropped_state(flat: &Path) {
    let git_dir = flat.join(".git");
    let hooks = git_dir.join("hooks");
    // Only custom (non-sample) hooks are worth flagging.
    let has_custom_hooks = fs::read_dir(&hooks)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .any(|e| !e.file_name().to_string_lossy().ends_with(".sample"))
        })
        .unwrap_or(false);
    if has_custom_hooks {
        warn!(
            "migrate: custom .git/hooks in '{}' are machine-local and will NOT be carried over",
            flat.display()
        );
    }
    warn!("migrate: machine-local state (extra .git/config remotes, alternates, reflogs) is not migrated");
}

/// Determine the REMOTE's default branch (not the local checked-out branch):
/// populate `origin/HEAD` from the remote, read it, and fall back to the
/// `clone.cfg` default only if the remote advertises none.
fn origin_default_branch(container: &Path, fallback: Option<&str>) -> Result<String> {
    let _ = git::run(&["remote", "set-head", "origin", "-a"], Some(container), None);
    let out = git::output(
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        Some(container),
        None,
    )?;
    if out.status.success() {
        let branch = out.stdout.trim().trim_start_matches("origin/").to_string();
        if !branch.is_empty() {
            return Ok(branch);
        }
    }
    if let Some(f) = fallback {
        warn!(
            "migrate: remote advertises no default branch; falling back to clone.cfg default '{}'",
            f
        );
        return Ok(f.to_string());
    }
    bail!(
        "could not determine the remote default branch for '{}'",
        container.display()
    )
}

/// Add the default-branch worktree, handling a default that exists only as a
/// remote-tracking ref (the flat repo may have deleted its local default).
fn add_default_worktree(container: &Path, branch: &str) -> Result<PathBuf> {
    if ref_exists(container, &format!("refs/heads/{}", branch)) {
        bare::add_worktree(container, branch)
    } else if ref_exists(container, &format!("refs/remotes/origin/{}", branch)) {
        let origin_ref = format!("origin/{}", branch);
        git::run(
            &["worktree", "add", "-b", branch, "--track", branch, &origin_ref],
            Some(container),
            None,
        )
        .wrap_err_with(|| format!("git worktree add --track {} in {}", branch, container.display()))?;
        Ok(container.join(branch))
    } else {
        bail!("default branch '{}' not found in the migrated repo", branch)
    }
}

/// Whether `refname` resolves in the container's git database.
fn ref_exists(container: &Path, refname: &str) -> bool {
    git::output(&["rev-parse", "--verify", "--quiet", refname], Some(container), None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `git worktree add <dir> <branch>` keeping the real branch name.
fn add_named_worktree(container: &Path, dir: &str, branch: &str) -> Result<PathBuf> {
    git::run(&["worktree", "add", dir, branch], Some(container), None)
        .wrap_err_with(|| format!("git worktree add {} {} in {}", dir, branch, container.display()))?;
    Ok(container.join(dir))
}

/// Repair worktree admin files after the container rename, passing each
/// worktree's new absolute path (proven necessary - the links are absolute).
fn repair_worktrees(container: &Path, staged_worktrees: &[PathBuf]) -> Result<()> {
    let names: Vec<String> = staged_worktrees
        .iter()
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .map(|n| container.join(n).to_string_lossy().into_owned())
        .collect();
    let mut args = vec!["worktree", "repair"];
    args.extend(names.iter().map(String::as_str));
    git::run(&args, Some(container), None).wrap_err("repairing worktree links after swap")
}

/// Verify the container resolves: `git status` succeeds AND reports a clean tree
/// in the worktree, and the origin URL matches what we captured.
fn verify(worktree: &Path, expected_origin: &str) -> Result<()> {
    let status = git::output(&["status", "--porcelain"], Some(worktree), None)?;
    if !status.stdout.trim().is_empty() {
        bail!(
            "verification failed: migrated worktree {} is not clean:\n{}",
            worktree.display(),
            status.stdout.trim()
        );
    }
    if !status.status.success() {
        bail!(
            "verification failed: 'git status' did not succeed in {}",
            worktree.display()
        );
    }
    let origin = git::output(&["remote", "get-url", "origin"], Some(worktree), None)?;
    if origin.stdout.trim() != expected_origin {
        bail!(
            "verification failed: origin is '{}', expected '{}'",
            origin.stdout.trim(),
            expected_origin
        );
    }
    Ok(())
}

/// `<parent>/<name>.<suffix>` next to `flat`.
fn sibling(flat: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = flat
        .parent()
        .ok_or_else(|| eyre!("'{}' has no parent directory", flat.display()))?;
    let name = flat
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre!("'{}' has no file name", flat.display()))?;
    Ok(parent.join(format!("{}.{}", name, suffix)))
}

/// Remove a directory, preferring `rkvr rmrf` (recoverable) and falling back to
/// std removal with a warning when rkvr is absent. A missing path is a no-op.
fn remove_dir(path: &Path) -> Result<()> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    match Command::new("rkvr").arg("rmrf").arg(path).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => bail!("rkvr rmrf {} failed: {}", path.display(), status),
        Err(_) => {
            warn!("rkvr not found; falling back to std removal of {}", path.display());
            fs::remove_dir_all(path).wrap_err_with(|| format!("removing {}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests;
