// common - bare-container primitives shared by `clone` and `worktree`.
//
// A bare container is `~/repos/<org>/<repo>/` holding `.bare/` (the git
// database), a `.git` pointer file, and one worktree directory per checked-out
// branch. These read-and-occasionally-mutate primitives are shared so the two
// binaries can't drift (notably `default_branch`, which mutates via
// `git remote set-head`, and `add_worktree`, the single guarded worktree-add).

use std::path::{Path, PathBuf};

use eyre::{Result, WrapErr, bail, eyre};
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

/// Whether `refname` resolves in the container's git database. The single home
/// for the check that used to be copy-pasted across `switch`, `migrate`, and
/// `prune`.
pub fn ref_exists(container: &Path, refname: &str) -> bool {
    debug!("ref_exists: container={:?} refname={}", container, refname);
    git::output(&["rev-parse", "--verify", "--quiet", refname], Some(container), None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One worktree entry from `git worktree list --porcelain`, unified across the
/// four ad-hoc parsers this replaces (`repo/info.rs`, `bare.rs`,
/// `clone/migrate.rs`, `worktree/list.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRow {
    /// Absolute path of the worktree's working directory.
    pub path: PathBuf,
    /// The checked-out branch, or `None` for a detached HEAD.
    pub branch: Option<String>,
    /// The worktree's HEAD sha; `None` when git reports none (the bare entry
    /// itself). Needed to rescue a detached-HEAD worktree.
    pub head: Option<String>,
    /// The bare repository entry itself (no working tree to `cd` into).
    pub bare: bool,
    /// A locked worktree (`git worktree lock`); prune/list must skip it.
    pub locked: bool,
}

/// Enumerate every worktree of `container` via one `git worktree list
/// --porcelain`, the single parser replacing four ad-hoc copies.
pub fn resolve_worktrees(container: &Path) -> Result<Vec<WorktreeRow>> {
    debug!("resolve_worktrees: container={:?}", container);
    let out = git::output(&["worktree", "list", "--porcelain"], Some(container), None)?;
    if !out.status.success() {
        bail!(
            "git worktree list failed in '{}': {}",
            container.display(),
            out.stderr.trim()
        );
    }
    let rows = parse_worktrees(&out.stdout);
    debug!("resolve_worktrees: found {} worktree(s)", rows.len());
    Ok(rows)
}

/// Parse the porcelain stream into rows. Blocks are separated by blank lines;
/// each opens with `worktree <path>`, then optional `HEAD <sha>` / `bare` /
/// `branch <ref>` / `detached` / `locked[ <reason>]` lines.
fn parse_worktrees(porcelain: &str) -> Vec<WorktreeRow> {
    let mut rows = Vec::new();
    let mut cur: Option<WorktreeRow> = None;

    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            if let Some(row) = cur.take() {
                rows.push(row);
            }
            cur = Some(WorktreeRow {
                path: PathBuf::from(rest.trim()),
                branch: None,
                head: None,
                bare: false,
                locked: false,
            });
        } else if let Some(row) = cur.as_mut() {
            if line == "bare" {
                row.bare = true;
            } else if let Some(sha) = line.strip_prefix("HEAD ") {
                row.head = Some(sha.trim().to_string());
            } else if let Some(refname) = line.strip_prefix("branch ") {
                row.branch = Some(refname.trim().trim_start_matches("refs/heads/").to_string());
            } else if line == "locked" || line.starts_with("locked ") {
                row.locked = true;
            }
            // `detached` carries no extra state beyond leaving `branch` unset.
        }
    }
    if let Some(row) = cur.take() {
        rows.push(row);
    }
    rows
}

/// Where a new worktree's content comes from.
pub enum Source<'a> {
    /// Check out an existing local branch as-is: `worktree add <dir> <branch>`.
    ExistingLocal,
    /// Create a local tracking branch from a remote ref:
    /// `worktree add -b <branch> --track <dir> <origin_ref>`.
    RemoteTracking { origin_ref: &'a str },
    /// Create a brand-new branch based on `base`:
    /// `worktree add -b <branch> <dir> <base>`.
    NewFrom { base: &'a str },
}

/// What to do when `branch` is already checked out, or the derived dir is taken.
pub enum Collision {
    /// Idempotent reuse: if `git worktree list` already has `branch` checked out
    /// anywhere, return that existing path (this is what makes a re-switch a no-op
    /// AND what keeps a legacy container whose worktree sits at a pre-slug raw
    /// path from triggering a fatal double-checkout). Otherwise, if the derived
    /// dir is occupied by an unrelated tree, bail. (interactive single-add -
    /// `worktree <branch>`)
    ReuseOrBail,
    /// Append a numeric suffix (`-1`, `-2`, …) until the dir is free, probed via
    /// `Path::exists()`. (batch recreation - `clone --migrate` linked worktrees)
    Uniquify,
}

/// A fully-resolved worktree-add request. The worktree directory is NOT a field:
/// the primitive always derives it as `slugify_branch(branch)`, then applies the
/// collision policy (which may append a suffix). Making the dir
/// underivable-by-the-caller is what keeps `clone` and `worktree` from drifting.
pub struct AddSpec<'a> {
    /// The branch the worktree hosts. For `ExistingLocal`/`RemoteTracking` this is
    /// the real branch name (e.g. `feature/auth`); for `NewFrom` it is the slug
    /// that also names the new branch (e.g. `new-feature`).
    pub branch: &'a str,
    pub source: Source<'a>,
    pub collision: Collision,
}

/// The guarded worktree-add primitive: one `git worktree add`, honoring the
/// collision policy. The directory is derived as `slugify_branch(spec.branch)`.
/// Returns the absolute path to the worktree (`container.join(final_dir)`),
/// including any `Uniquify` suffix.
pub fn add_worktree(container: &Path, spec: &AddSpec) -> Result<PathBuf> {
    debug!("add_worktree: container={:?} branch={}", container, spec.branch);

    let slug = git::slugify_branch(spec.branch);
    if slug.is_empty() {
        bail!(
            "branch name '{}' slugifies to empty; choose a name with alphanumerics",
            spec.branch
        );
    }

    let dir = match spec.collision {
        Collision::ReuseOrBail => {
            // Idempotent reuse / legacy-raw-path compatibility: if the branch is
            // already checked out ANYWHERE (by branch, not by derived dir), reuse
            // that existing path rather than attempting a second checkout (git
            // rejects an already-checked-out branch fatally).
            if let Some(existing) = worktree_path_for_branch(container, spec.branch)? {
                debug!(
                    "add_worktree: branch '{}' already checked out at {}; reusing",
                    spec.branch,
                    existing.display()
                );
                return Ok(existing);
            }
            // Branch is not checked out anywhere. If the derived dir is occupied by
            // an unrelated tree, bail rather than collide.
            let worktree = container.join(&slug);
            if worktree.exists() {
                bail!(
                    "'{}' already exists but does not host branch '{}' (slug collision); \
                     refusing to reuse it",
                    worktree.display(),
                    spec.branch
                );
            }
            slug
        }
        Collision::Uniquify => unique_dir(container, &slug),
    };

    let worktree = container.join(&dir);
    let add_args = build_add_args(&dir, spec);
    let arg_refs: Vec<&str> = add_args.iter().map(String::as_str).collect();
    git::run(&arg_refs, Some(container), None)
        .wrap_err_with(|| format!("git {:?} in {}", arg_refs, container.display()))?;
    Ok(worktree)
}

/// Build the `git worktree add` argument vector for `spec` targeting `dir`.
fn build_add_args(dir: &str, spec: &AddSpec) -> Vec<String> {
    match &spec.source {
        Source::ExistingLocal => vec![
            "worktree".to_string(),
            "add".to_string(),
            dir.to_string(),
            spec.branch.to_string(),
        ],
        Source::RemoteTracking { origin_ref } => vec![
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            spec.branch.to_string(),
            "--track".to_string(),
            dir.to_string(),
            origin_ref.to_string(),
        ],
        Source::NewFrom { base } => vec![
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            spec.branch.to_string(),
            dir.to_string(),
            base.to_string(),
        ],
    }
}

/// Probe `<base>`, `<base>-1`, `<base>-2`, … until a leaf dir in `container` is
/// free (via `Path::exists()`), returning the free leaf name.
fn unique_dir(container: &Path, base: &str) -> String {
    if !container.join(base).exists() {
        return base.to_string();
    }
    let mut n = 1;
    loop {
        let candidate = format!("{}-{}", base, n);
        if !container.join(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

/// The path of the worktree currently hosting `branch`, found via
/// `resolve_worktrees` (by branch, not by derived dir), or `None`.
fn worktree_path_for_branch(container: &Path, branch: &str) -> Result<Option<PathBuf>> {
    Ok(resolve_worktrees(container)?
        .into_iter()
        .find(|row| row.branch.as_deref() == Some(branch))
        .map(|row| row.path))
}

/// Ref-probing convenience for the `worktree` tool: take a raw branch string,
/// classify it (local / remote-only / new), and add with `Collision::ReuseOrBail`.
/// This is the relocated `switch` body.
pub fn resolve_and_add(container: &Path, raw_branch: &str, default_branch: Option<&str>) -> Result<PathBuf> {
    debug!(
        "resolve_and_add: container={:?} raw_branch={} default_branch={:?}",
        container, raw_branch, default_branch
    );

    // 1. Existing local branch → check it out as-is (real name; slugified dir).
    if ref_exists(container, &format!("refs/heads/{}", raw_branch)) {
        return add_worktree(
            container,
            &AddSpec {
                branch: raw_branch,
                source: Source::ExistingLocal,
                collision: Collision::ReuseOrBail,
            },
        );
    }

    // 2. Existing remote-only branch → create a tracking local branch (real name;
    //    slugified dir).
    if ref_exists(container, &format!("refs/remotes/origin/{}", raw_branch)) {
        let origin_ref = format!("origin/{}", raw_branch);
        return add_worktree(
            container,
            &AddSpec {
                branch: raw_branch,
                source: Source::RemoteTracking {
                    origin_ref: &origin_ref,
                },
                collision: Collision::ReuseOrBail,
            },
        );
    }

    // 3. New branch → slugify; the slug names both the branch and the dir, based
    //    on the default branch.
    let slug = git::slugify_branch(raw_branch);
    if slug.is_empty() {
        bail!(
            "branch name '{}' slugifies to empty; choose a name with alphanumerics",
            raw_branch
        );
    }
    let base = self::default_branch(container, default_branch)?;
    add_worktree(
        container,
        &AddSpec {
            branch: &slug,
            source: Source::NewFrom { base: &base },
            collision: Collision::ReuseOrBail,
        },
    )
}

#[cfg(test)]
mod tests;
