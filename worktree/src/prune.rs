// worktree — `--prune`: remove worktrees whose work is provably merged.
//
// Invariant that makes this safe: the branch *ref always survives* — prune only
// removes the checkout directory (`git worktree remove`), never a branch. A
// worktree is removed only when ALL of:
//   * it is not the default-branch worktree,
//   * it is not the worktree you are currently in,
//   * it is not locked,
//   * it has no uncommitted changes,
//   * its HEAD is an ancestor of `origin/<default>` (fully merged) — so nothing
//     unmerged is ever discarded.
// A removed worktree's commits live on in the bare DB and on `origin/<default>`,
// so recovery is a trivial `worktree <branch>` re-add. Removal is therefore
// `git worktree remove`, NOT rkvr: there is nothing unrecoverable to archive.
//
// Detection is against `origin/<default>` (a fetch advances it while local
// `<default>` lags). This does NOT fetch first, so it reflects the last fetch;
// run a fetch beforehand for the freshest picture.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use common::{bare, git};
use eyre::{Result, bail};
use log::debug;

use crate::list;

/// A worktree skipped by prune, with the human-readable reason.
struct Skipped {
    path: PathBuf,
    reason: &'static str,
}

/// Remove provably-merged, clean, unprotected worktrees in `container`.
/// Returns the paths actually removed. With `assume_yes` the confirmation prompt
/// is skipped (required in a non-interactive context).
pub fn prune(container: &Path, default_branch: Option<&str>, assume_yes: bool) -> Result<Vec<PathBuf>> {
    debug!("prune: container={:?} assume_yes={}", container, assume_yes);

    let default = bare::default_branch(container, default_branch)?;
    let origin_default = format!("origin/{}", default);
    if !bare::ref_exists(container, &format!("refs/remotes/{}", origin_default)) {
        bail!(
            "no '{}' ref to compare against; fetch first so prune can tell what is merged",
            origin_default
        );
    }

    let cwd = std::env::current_dir().ok().and_then(|p| std::fs::canonicalize(p).ok());

    let mut prunable: Vec<PathBuf> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();

    for entry in list::list(container)? {
        if entry.bare {
            continue;
        }
        // The default-branch worktree is a guaranteed invariant; never a target.
        if entry.branch.as_deref() == Some(default.as_str()) {
            continue;
        }
        let Some(_branch) = entry.branch.as_deref() else {
            skipped.push(Skipped {
                path: entry.path,
                reason: "detached HEAD",
            });
            continue;
        };
        if entry.locked {
            skipped.push(Skipped {
                path: entry.path,
                reason: "locked",
            });
            continue;
        }
        if is_current(&cwd, &entry.path) {
            skipped.push(Skipped {
                path: entry.path,
                reason: "current worktree",
            });
            continue;
        }
        if is_dirty(&entry.path)? {
            skipped.push(Skipped {
                path: entry.path,
                reason: "uncommitted changes",
            });
            continue;
        }
        if is_merged(&entry.path, &origin_default)? {
            prunable.push(entry.path);
        } else {
            skipped.push(Skipped {
                path: entry.path,
                reason: "not merged",
            });
        }
    }

    report(&prunable, &skipped, &origin_default);

    if prunable.is_empty() {
        return Ok(Vec::new());
    }
    if !confirm(prunable.len(), assume_yes)? {
        eprintln!("worktree: aborted; nothing removed");
        return Ok(Vec::new());
    }

    let mut removed = Vec::new();
    for path in prunable {
        // Clean + merged, so a plain remove is safe and the branch ref survives.
        match git::run(&["worktree", "remove", &path.to_string_lossy()], Some(container), None) {
            Ok(()) => removed.push(path),
            Err(e) => eprintln!("worktree: failed to remove {}: {}", path.display(), e),
        }
    }
    // Tidy any now-stale admin entries (e.g. hand-deleted dirs).
    let _ = git::run(&["worktree", "prune"], Some(container), None);

    Ok(removed)
}

/// Whether the current directory is inside `worktree` (so we must not remove it).
fn is_current(cwd: &Option<PathBuf>, worktree: &Path) -> bool {
    match (cwd, std::fs::canonicalize(worktree).ok()) {
        (Some(cwd), Some(wt)) => cwd.starts_with(&wt),
        _ => false,
    }
}

/// Whether `worktree` has uncommitted changes.
fn is_dirty(worktree: &Path) -> Result<bool> {
    let out = git::output(&["status", "--porcelain"], Some(worktree), None)?;
    Ok(!out.stdout.trim().is_empty())
}

/// Whether `worktree`'s HEAD is an ancestor of `origin_default` (fully merged,
/// so nothing is lost by removing the checkout).
fn is_merged(worktree: &Path, origin_default: &str) -> Result<bool> {
    let out = git::output(
        &["merge-base", "--is-ancestor", "HEAD", origin_default],
        Some(worktree),
        None,
    )?;
    Ok(out.status.success())
}

/// Print what will be removed and what was skipped, to stderr (stdout carries no
/// path for the prune form — it runs through the shell wrapper untouched).
fn report(prunable: &[PathBuf], skipped: &[Skipped], origin_default: &str) {
    if prunable.is_empty() {
        eprintln!("worktree: nothing to prune (no clean worktrees merged into {origin_default})");
    } else {
        eprintln!("worktree: merged into {origin_default}, will remove (branch refs kept):");
        for path in prunable {
            eprintln!("  {}", path.display());
        }
    }
    if !skipped.is_empty() {
        eprintln!("worktree: keeping:");
        for s in skipped {
            eprintln!("  {} ({})", s.path.display(), s.reason);
        }
    }
}

/// Confirm removal: `assume_yes` short-circuits; otherwise prompt on an
/// interactive terminal; refuse in a non-interactive context without `--yes`.
fn confirm(count: usize, assume_yes: bool) -> Result<bool> {
    if assume_yes {
        return Ok(true);
    }
    if !std::io::stdin().is_terminal() {
        bail!("worktree: refusing to prune {count} worktree(s) non-interactively; pass --yes");
    }
    eprint!("Remove {count} worktree(s)? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "Yes"))
}

#[cfg(test)]
mod tests;
