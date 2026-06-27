// clone — `--migrate`: convert a flat checkout into a bare container without
// losing git-tracked local work.
//
// The original tree is never mutated destructively until a verified, recoverable
// swap: the bare container is built from the LOCAL repo (preserving unpushed
// commits and local-only branches), staged alongside as `<repo>.migrating`,
// verified, then rename-swapped (`<repo>` → `<repo>.backup` → `<repo>.migrating`
// → `<repo>`), re-verified, and only then is the backup removed (via
// `rkvr rmrf`, recoverable until rkvr harvests it).
//
// Before staging, a read-only PREFLIGHT runs (require rkvr, resolve the per-org
// SSH key, probe remote connectivity, enumerate the worktree set). Then an
// additive RESCUE pass materializes every dirty tree, stash, and detached-HEAD
// worktree as a `wip/*` branch so the committed-refs-only bare clone strands
// nothing. The rescue only ADDS refs and moves dirty work into stashes; it never
// rewrites or deletes committed history.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::git;
use eyre::{Result, WrapErr, bail, eyre};
use log::{debug, warn};

use crate::bare;

/// Max length of the slug portion of a `wip/*` rescue branch (git's refname
/// limit is 255; keep well under it so the `wip/` prefix + suffixes always fit).
const WIP_SLUG_MAX: usize = 80;

/// One entry from `git worktree list --porcelain`. The first entry returned by
/// [`list_worktrees`] is always the main worktree (the flat checkout itself);
/// the rest are linked worktrees.
struct Worktree {
    /// Absolute path of the worktree's working directory.
    path: PathBuf,
    /// The checked-out branch, or `None` when detached.
    branch: Option<String>,
    /// The worktree's HEAD sha (needed to rescue a detached worktree).
    head: String,
}

/// Resolve the flat checkout to migrate from the current directory.
pub fn flat_from_cwd() -> Result<PathBuf> {
    let cwd = std::env::current_dir().wrap_err("determining current directory")?;
    flat_from_dir(&cwd)
}

/// Resolve the flat checkout to migrate from `dir`: the MAIN worktree of the
/// enclosing repo, so it works from a subdirectory or even a legacy linked
/// worktree. Rejects an already-migrated bare container. Split from
/// `flat_from_cwd` so it is testable without mutating the process cwd.
fn flat_from_dir(dir: &Path) -> Result<PathBuf> {
    debug!("flat_from_dir: dir={:?}", dir);
    let out = git::output(&["worktree", "list", "--porcelain"], Some(dir), None)?;
    if !out.status.success() {
        bail!(
            "not inside a git checkout - run --migrate from within the flat checkout you want to convert, or pass org/repo"
        );
    }
    // A bare repo (clone's already-migrated layout) emits a `bare` line.
    if out.stdout.lines().any(|l| l.trim() == "bare") {
        bail!("the enclosing repo is already a bare container; nothing to migrate");
    }
    // The first `worktree <path>` line is the main worktree - true from a
    // subdirectory or a linked worktree alike (verified against git porcelain).
    let main = out
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("worktree "))
        .map(|p| PathBuf::from(p.trim()))
        .ok_or_else(|| eyre!("could not resolve the main worktree from {}", dir.display()))?;
    debug!("flat_from_dir: main worktree={:?}", main);
    Ok(main)
}

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
    // Absolutize once so the post-swap `git worktree repair` can never disagree
    // with the cwd it runs in (the relative-clonepath repair bug). Every path
    // derived below inherits this absolute base.
    let flat = flat
        .canonicalize()
        .wrap_err_with(|| format!("resolving absolute path of {}", flat.display()))?;
    let flat = flat.as_path();

    // 1. PREFLIGHT (read-only; any failure here leaves the repo unchanged).
    require_rkvr()?;
    let origin = origin_url(flat)?;
    let ssh_owned = ssh_env_for_origin(&origin);
    let ssh_borrow: Option<Vec<(&str, &str)>> = ssh_owned
        .as_ref()
        .map(|v| v.iter().map(|(k, val)| (k.as_str(), val.as_str())).collect());
    let ssh = ssh_borrow.as_deref();
    check_connectivity(flat, ssh)?;
    let worktrees = list_worktrees(flat)?;

    // 2. RESCUE PASS (additive: only ADDS wip/* refs + moves dirty work to
    //    stashes; never rewrites/deletes a commit or branch). Bails BEFORE any
    //    mutation if a tree is mid-merge/unmerged.
    let wip_branches = rescue_work(flat, &worktrees)?;

    // 3. After rescue every tree must be clean (loop the FULL set, not just main).
    assert_all_clean(&worktrees)?;

    // 4. Capture the currently checked-out branch and warn about dropped state.
    let current = current_branch(flat);
    warn_dropped_state(flat);
    if !wip_branches.is_empty() {
        eprintln!(
            "migrate: rescued in-flight work to branches: {}",
            wip_branches.join(", ")
        );
    }

    // 5. Clone the bare container from the LOCAL repo (captures every local ref
    //    at its local state - unpushed commits, local-only branches, wip/*).
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

    // 6. Repoint at the real remote, then refspec fix + fetch (with the resolved
    //    SSH key) - updates refs/remotes/origin/* only, preserving local-ahead
    //    refs/heads/*.
    git::run(&["remote", "set-url", "origin", &origin], Some(&bare), None)?;
    bare::write_git_pointer(&migrating)?;
    bare::fix_fetch_refspec(&migrating, ssh)?;

    // 7. Determine the TRUE default branch from the REMOTE, create the
    //    always-present default-branch worktree, reset the container HEAD to it,
    //    then add the previously checked-out branch's worktree when it differs.
    let default = origin_default_branch(&migrating, default_fallback, ssh)?;
    let mut worktree_paths = vec![add_default_worktree(&migrating, &default)?];
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
        worktree_paths.push(add_named_worktree(&migrating, &dir, cur)?);
    }

    // 8. Verify the staged container, then perform the recoverable swap.
    verify(&migrating.join(&default), &origin)?;

    let backup = sibling(flat, "backup")?;
    remove_dir(&backup)?;
    fs::rename(flat, &backup).wrap_err_with(|| format!("renaming {} aside", flat.display()))?;
    if let Err(e) = fs::rename(&migrating, flat) {
        // Swap-in failed: restore the original from backup.
        let _ = fs::rename(&backup, flat);
        return Err(e).wrap_err("swapping the migrated container into place failed");
    }

    // 9. Worktree admin files store absolute paths recorded at the staging path;
    //    repair them to the final location, then re-verify. A failure in EITHER
    //    step rolls back to the original (backup).
    let final_worktree = flat.join(&default);
    if let Err(e) = repair_worktrees(flat, &worktree_paths).and_then(|()| verify(&final_worktree, &origin)) {
        let _ = remove_dir(flat);
        let _ = fs::rename(&backup, flat);
        return Err(e).wrap_err("migrated container failed repair/verification after swap");
    }

    // 10. Migration committed; remove the backup (recoverable via rkvr rmrf).
    if let Err(e) = remove_dir(&backup) {
        warn!("migrate: could not remove backup {}: {}", backup.display(), e);
    }

    Ok(final_worktree)
}

/// Refuse to run without `rkvr`: migrate's removals must be recoverable, never a
/// raw non-recoverable delete (the project's hard safety rule).
fn require_rkvr() -> Result<()> {
    match Command::new("rkvr").arg("--version").output() {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!("`rkvr` is required for --migrate (its removals must be recoverable); install it and re-run"),
    }
}

/// Resolve the per-org SSH key for the origin URL as a `GIT_SSH_COMMAND` env
/// override to apply to network ops. `None` means "use ambient SSH".
fn ssh_env_for_origin(origin: &str) -> Option<Vec<(String, String)>> {
    let org = git::parse_repospec(origin).ok().map(|s| s.org)?;
    match crate::config::find_ssh_key_for_org(&org) {
        Ok(Some(key)) => Some(vec![("GIT_SSH_COMMAND".to_string(), git::ssh_command(&key))]),
        Ok(None) => None,
        Err(e) => {
            warn!(
                "migrate: could not resolve SSH key for org '{}': {}; using ambient SSH",
                org, e
            );
            None
        }
    }
}

/// Probe remote reachability BEFORE any mutation, so a credential/network
/// failure surfaces before the rescue pass touches the repo.
fn check_connectivity(flat: &Path, ssh: Option<&[(&str, &str)]>) -> Result<()> {
    debug!("check_connectivity: flat={:?}", flat);
    let out = git::output(&["ls-remote", "--quiet", "origin"], Some(flat), ssh)?;
    if !out.status.success() {
        bail!(
            "cannot reach 'origin' before migrating '{}': {}\n\
             Fix connectivity/credentials and re-run; nothing has been changed.",
            flat.display(),
            out.stderr.trim()
        );
    }
    Ok(())
}

/// Parse `git worktree list --porcelain` into the main worktree plus every
/// linked worktree, capturing path, branch, and HEAD sha.
fn list_worktrees(flat: &Path) -> Result<Vec<Worktree>> {
    debug!("list_worktrees: flat={:?}", flat);
    let out = git::output(&["worktree", "list", "--porcelain"], Some(flat), None)?;
    if !out.status.success() {
        bail!(
            "could not list worktrees for '{}': {}",
            flat.display(),
            out.stderr.trim()
        );
    }

    let mut worktrees = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut head = String::new();
    let flush =
        |worktrees: &mut Vec<Worktree>, path: &mut Option<PathBuf>, branch: &mut Option<String>, head: &mut String| {
            if let Some(p) = path.take() {
                worktrees.push(Worktree {
                    path: p,
                    branch: branch.take(),
                    head: std::mem::take(head),
                });
            }
        };
    for line in out.stdout.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            flush(&mut worktrees, &mut path, &mut branch, &mut head);
            path = Some(PathBuf::from(p.trim()));
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            head = h.trim().to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = Some(b.trim().trim_start_matches("refs/heads/").to_string());
        } else if line.trim().is_empty() {
            flush(&mut worktrees, &mut path, &mut branch, &mut head);
        }
    }
    flush(&mut worktrees, &mut path, &mut branch, &mut head);
    debug!("list_worktrees: found {} worktree(s)", worktrees.len());
    Ok(worktrees)
}

/// Auto-rescue every dirty worktree, stash, and detached-HEAD worktree into a
/// `wip/*` branch so the committed-refs-only bare clone strands nothing. Returns
/// the names of the branches created. Additive: never rewrites/deletes a commit
/// or branch.
fn rescue_work(flat: &Path, worktrees: &[Worktree]) -> Result<Vec<String>> {
    debug!("rescue_work: flat={:?} worktrees={}", flat, worktrees.len());

    // 0. Bail BEFORE any mutation if a tree is mid-merge / has unmerged paths -
    //    `git stash` would be fatal and leave a half-rescued repo.
    for wt in worktrees {
        let unmerged = git::output(&["diff", "--name-only", "--diff-filter=U"], Some(&wt.path), None)?;
        if !unmerged.stdout.trim().is_empty() {
            bail!(
                "refusing to migrate: worktree {} has unmerged paths (mid-merge/rebase). \
                 Resolve or abort it, then re-run --migrate. Nothing has been changed.",
                wt.path.display()
            );
        }
    }

    // Seed the name set with existing local branches so a wip/* name never
    // collides (prefix-aware: see wip_branch_name).
    let existing = git::output(&["branch", "--format=%(refname:short)"], Some(flat), None)?;
    let mut used: HashSet<String> = existing.stdout.lines().map(|l| l.trim().to_string()).collect();
    let mut created = Vec::new();

    // 1. Detached-HEAD worktrees: their commit physically survives the local
    //    clone but is unreferenced/GC-eligible, so give it a wip/* ref.
    for wt in worktrees {
        if wt.branch.is_none() && !wt.head.is_empty() {
            let short: String = wt.head.chars().take(12).collect();
            let name = wip_branch_name(&format!("detached-{}", short), &mut used);
            git::run(&["branch", &name, &wt.head], Some(flat), None)
                .wrap_err_with(|| format!("rescuing detached worktree {} to {}", wt.path.display(), name))?;
            debug!("rescue_work: detached {} -> {}", wt.path.display(), name);
            created.push(name);
        }
    }

    // 2. Stash each dirty worktree (tracked + untracked) onto the shared stack.
    for wt in worktrees {
        let status = git::output(&["status", "--porcelain"], Some(&wt.path), None)?;
        if status.stdout.trim().is_empty() {
            continue;
        }
        let label = wt.branch.as_deref().unwrap_or("detached");
        debug!("rescue_work: stashing dirty worktree {:?} (branch {})", wt.path, label);
        git::run(
            &[
                "stash",
                "push",
                "--include-untracked",
                "-m",
                &format!("migrate-rescue: {}", label),
            ],
            Some(&wt.path),
            None,
        )
        .wrap_err_with(|| format!("auto-stashing dirty worktree {}", wt.path.display()))?;
    }

    // 3. Convert every stash entry (pre-existing + just-pushed) into a wip/*
    //    branch. The bare clone never copies refs/stash, so the branch is what
    //    carries the work forward.
    let list = git::output(&["stash", "list"], Some(flat), None)?;
    let entries: Vec<String> = list.stdout.lines().map(str::to_string).collect();
    for (i, entry) in entries.iter().enumerate() {
        let message = entry.split_once(": ").map(|(_, m)| m).unwrap_or("");
        let slug = git::slugify_branch(message);
        let slug = if slug.is_empty() { format!("stash-{}", i) } else { slug };
        let name = wip_branch_name(&slug, &mut used);
        let stash_ref = format!("stash@{{{}}}", i);
        git::run(&["branch", &name, &stash_ref], Some(flat), None)
            .wrap_err_with(|| format!("rescuing {} to branch {}", stash_ref, name))?;
        debug!("rescue_work: {} -> {}", stash_ref, name);
        created.push(name);
    }
    Ok(created)
}

/// A collision-free `wip/<slug>` branch name. Length-capped, and **prefix-aware**:
/// in refs/heads/ a name and a path-prefix of it are mutually exclusive
/// (`wip/foo` vs `wip/foo/bar`, or a bare `wip`), so a candidate is rejected if
/// it path-conflicts with any name already in `used`. Our slugs are single
/// segments (`slugify_branch` collapses `/`), so only `used` membership matters.
fn wip_branch_name(slug: &str, used: &mut HashSet<String>) -> String {
    let mut capped: String = slug.chars().take(WIP_SLUG_MAX).collect();
    capped = capped.trim_matches('-').to_string();
    if capped.is_empty() {
        capped = "rescue".to_string();
    }
    let base = format!("wip/{}", capped);
    let mut name = base.clone();
    let mut n = 1;
    while path_conflicts(&name, used) {
        name = format!("{}-{}", base, n);
        n += 1;
    }
    used.insert(name.clone());
    name
}

/// Whether `candidate` collides with any existing ref name under refs/heads/ -
/// either an exact match or a directory/file path-prefix conflict.
fn path_conflicts(candidate: &str, used: &HashSet<String>) -> bool {
    used.iter().any(|e| {
        e == candidate || e.starts_with(&format!("{}/", candidate)) || candidate.starts_with(&format!("{}/", e))
    })
}

/// Assert every worktree's tree is clean after rescue (rescue should make this
/// hold; bail loudly rather than bare-clone over uncommitted work).
fn assert_all_clean(worktrees: &[Worktree]) -> Result<()> {
    for wt in worktrees {
        let status = git::output(&["status", "--porcelain"], Some(&wt.path), None)?;
        if !status.stdout.trim().is_empty() {
            bail!(
                "worktree {} is still not clean after rescue:\n{}",
                wt.path.display(),
                status.stdout.trim()
            );
        }
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
fn origin_default_branch(container: &Path, fallback: Option<&str>, ssh: Option<&[(&str, &str)]>) -> Result<String> {
    let _ = git::run(&["remote", "set-head", "origin", "-a"], Some(container), ssh);
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

/// Remove a directory recoverably via `rkvr rmrf`. `rkvr` presence is enforced
/// by `require_rkvr` in preflight, so a missing rkvr here is an error, never a
/// silent non-recoverable delete. A missing path is a no-op.
fn remove_dir(path: &Path) -> Result<()> {
    if path.symlink_metadata().is_err() {
        return Ok(());
    }
    match Command::new("rkvr").arg("rmrf").arg(path).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => bail!("rkvr rmrf {} failed: {}", path.display(), status),
        Err(e) => bail!("rkvr rmrf {} could not run: {}", path.display(), e),
    }
}

#[cfg(test)]
mod tests;
