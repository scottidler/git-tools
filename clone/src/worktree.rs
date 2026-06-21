// clone — `--worktree <branch>`: add a worktree to an existing bare container.

use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, WrapErr, bail};
use log::debug;

use crate::bare;
use crate::config::Config;

/// Add (or reuse) a worktree for `raw_branch` in the bare container resolved
/// from `config`, returning the worktree path the wrapper `cd`s into.
///
/// Branch-source selection uses the **raw** argument first so existing
/// non-slug branches stay reachable; only a brand-new branch is slugified.
pub fn add(config: &Config, raw_branch: &str) -> Result<PathBuf> {
    let container = resolve_container(config)?;
    debug!("worktree::add: container={:?} raw_branch={}", container, raw_branch);

    if !bare::is_bare_container(&container) {
        bail!(
            "'{}' is not a bare container; --worktree requires the bare layout (run `clone --migrate` first)",
            container.display()
        );
    }

    // 1. Existing local branch → check it out as-is into a slugified dir.
    if ref_exists(&container, &format!("refs/heads/{}", raw_branch)) {
        let dir = git::slugify_branch(raw_branch);
        return ensure_or_add(&container, &dir, &["worktree", "add", &dir, raw_branch]);
    }

    // 2. Existing remote branch → create a tracking local branch (real name),
    //    slugified dir.
    if ref_exists(&container, &format!("refs/remotes/origin/{}", raw_branch)) {
        let dir = git::slugify_branch(raw_branch);
        let origin_ref = format!("origin/{}", raw_branch);
        return ensure_or_add(
            &container,
            &dir,
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
    let base = bare::default_branch(&container, config.default_branch.as_deref())?;
    ensure_or_add(&container, &slug, &["worktree", "add", "-b", &slug, &slug, &base])
}

/// Resolve the bare container: an explicit `org/repo` arg → `clonepath/org/repo`;
/// otherwise derive it from CWD via `git rev-parse --git-common-dir` (which
/// returns `<container>/.bare` from anywhere inside any worktree) and take its
/// parent.
fn resolve_container(config: &Config) -> Result<PathBuf> {
    if let Some(spec) = &config.spec {
        return Ok(config.clonepath.join(spec.to_string()));
    }

    let out = git::output(&["rev-parse", "--git-common-dir"], None, None)?;
    if !out.status.success() {
        bail!("not inside a git repository; run --worktree inside a bare container or pass org/repo");
    }

    // `--git-common-dir` may be relative to CWD; canonicalize then take the
    // parent of `.bare`.
    let common_dir = std::fs::canonicalize(out.stdout.trim())
        .wrap_err_with(|| format!("resolving git common dir '{}'", out.stdout.trim()))?;
    common_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| eyre::eyre!("git common dir '{}' has no parent", common_dir.display()))
}

/// Add the worktree unless its directory already exists (idempotent: a re-run
/// just `cd`s into the existing worktree). Returns the worktree path.
fn ensure_or_add(container: &Path, dir: &str, add_args: &[&str]) -> Result<PathBuf> {
    let worktree = container.join(dir);
    if worktree.is_dir() {
        debug!("ensure_or_add: '{}' already exists; reusing", worktree.display());
        return Ok(worktree);
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
