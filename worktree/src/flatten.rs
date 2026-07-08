// worktree — `flatten`: collapse a gratuitous bare container back into a flat
// checkout without losing any ref-reachable or per-worktree work.
//
// This is the reverse of `migrate`, and it is deliberately REFUSE-FIRST rather
// than auto-rescuing: a structural, data-loss-prone collapse should stop and make
// the user commit/push/prune unsafe state, not silently stash it (the auto-stash
// is exactly what made the first design draft self-contradictory). The invariant:
//
//   the collapse preserves every ref under `refs/*` at an identical OID, AND any
//   per-worktree state not represented by a preserved ref BLOCKS the collapse.
//
// Because the whole original container is archived via `rkvr` before the swap, a
// removed worktree's ignored files (`.env`, build state) stay recoverable; they
// are reported, never a silent drop.
//
// The DB transition is copy-based so the live `.bare` is never mutated in place:
// `.bare/` is copied to `<repo>.flattening/.git/`, converted to a non-bare repo,
// its tree materialized and verified, then rename-swapped
// (`<repo>` → `<repo>.flatten-backup` → `<repo>.flattening` → `<repo>`),
// re-verified, and only then is the backup removed (recoverably, via `rkvr rmrf`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, WrapErr, bail, eyre};
use log::{debug, warn};

use crate::bare;

/// The read-only result of inspecting a container for collapse. `refusals` is the
/// list of refuse-first reasons (empty = safe to collapse); `ignored` records, per
/// worktree, the git-ignored paths that will be archived (recoverable) rather than
/// carried into the flat checkout.
struct Inspection {
    /// The container's default branch (the flat checkout will land on it).
    default: String,
    /// Every ref under `refs/*` in the live container, `refname` → `objectname`.
    /// The retention proof asserts this set survives the transition unchanged.
    refs_before: BTreeMap<String, String>,
    /// Reasons the collapse must refuse. Empty means safe.
    refusals: Vec<String>,
    /// Per-worktree git-ignored paths (recoverable from the rkvr archive).
    ignored: Vec<(PathBuf, Vec<String>)>,
}

/// Resolve the bare container to flatten from the current directory, so
/// `worktree flatten` works from a linked worktree, a subdirectory, or the
/// container root alike.
pub fn container_from_cwd() -> Result<PathBuf> {
    let cwd = std::env::current_dir().wrap_err("determining current directory")?;
    container_from_dir(&cwd)
}

/// Resolve the enclosing bare container from `dir`. Split from
/// [`container_from_cwd`] so it is testable without mutating the process cwd.
/// Rejects a flat checkout (nothing to flatten).
fn container_from_dir(dir: &Path) -> Result<PathBuf> {
    debug!("container_from_dir: dir={:?}", dir);
    let out = git::output(
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        Some(dir),
        None,
    )?;
    if !out.status.success() {
        bail!(
            "not inside a git repository - run `worktree flatten` from within the bare container you want to collapse, or pass org/repo"
        );
    }
    let common = PathBuf::from(out.stdout.trim());
    let container = common
        .parent()
        .ok_or_else(|| eyre!("could not resolve container from git-common-dir {}", common.display()))?
        .to_path_buf();
    if !bare::is_bare_container(&container) {
        bail!(
            "'{}' is not a bare container; nothing to flatten (only a bare container collapses to flat)",
            container.display()
        );
    }
    debug!("container_from_dir: container={:?}", container);
    Ok(container)
}

/// Collapse the bare container at `container` into a flat checkout in place,
/// returning the resulting flat checkout path. `default_fallback` is the
/// config `default` branch, used only if the container advertises no
/// default branch. Refuses (before any mutation) on any unsafe worktree state.
pub fn flatten(container: &Path, default_fallback: Option<&str>) -> Result<PathBuf> {
    debug!("flatten: container={:?}", container);

    if !bare::is_bare_container(container) {
        bail!("'{}' is not a bare container", container.display());
    }
    let container = container
        .canonicalize()
        .wrap_err_with(|| format!("resolving absolute path of {}", container.display()))?;
    let container = container.as_path();

    // 1. PREFLIGHT (read-only). rkvr must be present so every removal below is
    //    recoverable; then inspect refuses on any unsafe/unmergeable state.
    common::rkvr::require()?;
    let inspection = inspect(container, default_fallback)?;

    if !inspection.refusals.is_empty() {
        bail!(
            "refusing to flatten '{}': {} unsafe worktree condition(s) found (commit/push/prune, then re-run):\n  - {}",
            container.display(),
            inspection.refusals.len(),
            inspection.refusals.join("\n  - ")
        );
    }

    // Ignored files are local user data: they are archived with the whole
    // container below (recoverable), never silently dropped. Report them.
    report_ignored(&inspection.ignored);

    // 2. COPY-BASED CRASH-SAFE TRANSITION (live `.bare` untouched until swap).
    let flat = perform_transition(container, &inspection.default, &inspection.refs_before)?;

    eprintln!(
        "Flattened '{}' to a single flat checkout on '{}'.",
        flat.display(),
        inspection.default
    );
    if !inspection.ignored.is_empty() {
        eprintln!(
            "  Git-ignored files in the removed worktrees were archived with the container (recover with `rkvr rcvr`)."
        );
    }
    Ok(flat)
}

/// Preview a collapse without changing anything: run the read-only inspection and
/// print the plan (default branch, retained ref count, refusals, ignored files) to
/// STDERR. Returns the container path so the wrapper leaves the user in place.
pub fn dry_run(container: &Path, default_fallback: Option<&str>) -> Result<PathBuf> {
    debug!("dry_run: container={:?}", container);

    if !bare::is_bare_container(container) {
        bail!("'{}' is not a bare container", container.display());
    }
    let container = container
        .canonicalize()
        .wrap_err_with(|| format!("resolving absolute path of {}", container.display()))?;
    let container = container.as_path();

    let rkvr_ok = common::rkvr::require().is_ok();
    let inspection = inspect(container, default_fallback)?;

    eprintln!(
        "DRY RUN: flatten '{}' -> flat checkout on '{}' (no changes made)",
        container.display(),
        inspection.default
    );
    if !rkvr_ok {
        eprintln!("  WARNING: rkvr not found - the real run would abort in preflight");
    }
    eprintln!(
        "  retained refs: {} under refs/* preserved at identical OIDs",
        inspection.refs_before.len()
    );

    if inspection.refusals.is_empty() {
        eprintln!("  => safe to collapse; run again without --dry-run to perform the flatten");
    } else {
        eprintln!(
            "  => flatten WOULD REFUSE ({} unsafe condition(s)); resolve, then re-run:",
            inspection.refusals.len()
        );
        for reason in &inspection.refusals {
            eprintln!("    - {}", reason);
        }
    }

    for (wt, files) in &inspection.ignored {
        if !files.is_empty() {
            eprintln!(
                "  git-ignored files in {} (archived with the container, recoverable): {}",
                wt.display(),
                files.join(", ")
            );
        }
    }
    Ok(container.to_path_buf())
}

/// Read-only inspection: enumerate the container's worktrees and refs, then apply
/// every refuse-first check, collecting reasons rather than bailing (so both the
/// real run and `--dry-run` share one check body; the real run bails on a
/// non-empty `refusals`). Only genuine errors (git failures, no default branch)
/// bail here.
fn inspect(container: &Path, default_fallback: Option<&str>) -> Result<Inspection> {
    debug!("inspect: container={:?}", container);

    let default = bare::default_branch(container, default_fallback)?;
    let refs_before = refs_snapshot(container)?;
    debug!("inspect: default={} refs_before={}", default, refs_before.len());

    let rows = common::bare::resolve_worktrees(container)?;
    let mut refusals = Vec::new();
    let mut ignored = Vec::new();

    // Container-level: an existing stash is per-repo state with no preserved ref
    // (the bare clone never copies `refs/stash` forward), so it blocks the collapse.
    if bare::ref_exists(container, "refs/stash") {
        refusals.push("an existing `refs/stash` would be lost (pop/drop it first)".to_string());
    }

    for row in rows.iter().filter(|r| !r.bare) {
        let wt = row.path.as_path();
        debug!("inspect: worktree {:?} branch={:?}", wt, row.branch);

        // Uncommitted changes or untracked files. A check that itself ERRORS
        // leaves the worktree's safety UNDETERMINED, so it must BLOCK the collapse
        // (fail-closed) - `reset --hard` would otherwise be free to discard work
        // that no `refs/*` retention proof guards.
        match common::bare::is_dirty(wt) {
            Ok(true) => refusals.push(format!(
                "worktree {} has uncommitted changes or untracked files",
                wt.display()
            )),
            Ok(false) => {}
            Err(e) => refusals.push(format!(
                "worktree {} could not be checked for uncommitted changes ({}); refusing (fail-closed)",
                wt.display(),
                e
            )),
        }

        // Active merge / rebase / cherry-pick / revert / bisect in the worktree's
        // private gitdir (not a WorktreeRow field). If the gitdir cannot be
        // resolved, BOTH the in-progress-operation and the per-worktree-config /
        // sparse-checkout checks are unverifiable - fail-closed rather than
        // silently skip them.
        match worktree_gitdir(wt) {
            Ok(gitdir) => {
                if let Some(op) = in_progress_operation(&gitdir) {
                    refusals.push(format!(
                        "worktree {} has an in-progress {} (finish/abort it first)",
                        wt.display(),
                        op
                    ));
                }
                if let Some(kind) = per_worktree_state(&gitdir) {
                    refusals.push(format!(
                        "worktree {} carries {} that a flat checkout cannot represent",
                        wt.display(),
                        kind
                    ));
                }
            }
            Err(e) => refusals.push(format!(
                "worktree {} git dir could not be resolved ({}); in-progress-operation and per-worktree-state checks are unverifiable; refusing (fail-closed)",
                wt.display(),
                e
            )),
        }

        // Dirty / conflicted submodules. An undeterminable submodule state (git
        // erroring, not merely "no submodules") must BLOCK the collapse.
        match dirty_submodules(wt) {
            Ok(subs) if !subs.is_empty() => refusals.push(format!(
                "worktree {} has dirty/conflicted submodule(s): {}",
                wt.display(),
                subs.join(", ")
            )),
            Ok(_) => {}
            Err(e) => refusals.push(format!(
                "worktree {} submodule state could not be determined ({}); refusing (fail-closed)",
                wt.display(),
                e
            )),
        }

        // Branch ancestry / detached-HEAD reachability. A worktree branch with
        // commits not in the default, or a detached HEAD unreachable from any
        // preserved ref, would strand work on collapse.
        match &row.branch {
            Some(branch) => {
                if !is_ancestor(
                    container,
                    &format!("refs/heads/{}", branch),
                    &format!("refs/heads/{}", default),
                ) {
                    refusals.push(format!(
                        "worktree {} is on '{}', which is not merged into the default '{}' (push/merge/prune it first)",
                        wt.display(),
                        branch,
                        default
                    ));
                }
            }
            None => {
                let head = row.head.as_deref().unwrap_or("");
                if head.is_empty() || !commit_reachable_from_ref(container, head) {
                    refusals.push(format!(
                        "worktree {} is a detached HEAD ({}) not reachable from any ref (branch it first)",
                        wt.display(),
                        if head.is_empty() { "unknown" } else { head }
                    ));
                }
            }
        }

        // Ignored files (reported as recoverable, never a refuse).
        let files = ignored_files(wt);
        if !files.is_empty() {
            ignored.push((wt.to_path_buf(), files));
        }
    }

    debug!(
        "inspect: {} refusal(s), {} worktree(s) with ignored files",
        refusals.len(),
        ignored.len()
    );
    Ok(Inspection {
        default,
        refs_before,
        refusals,
        ignored,
    })
}

/// Snapshot every ref under `refs/*` as `refname` → `objectname`, via one
/// `git for-each-ref`. A `BTreeMap` keeps the ordering deterministic so the
/// before/after retention comparison is stable.
fn refs_snapshot(dir: &Path) -> Result<BTreeMap<String, String>> {
    debug!("refs_snapshot: dir={:?}", dir);
    let out = git::output(&["for-each-ref", "--format=%(refname) %(objectname)"], Some(dir), None)?;
    if !out.status.success() {
        bail!("git for-each-ref failed in '{}': {}", dir.display(), out.stderr.trim());
    }
    let mut refs = BTreeMap::new();
    for line in out.stdout.lines() {
        if let Some((name, oid)) = line.trim().split_once(' ') {
            refs.insert(name.to_string(), oid.to_string());
        }
    }
    debug!("refs_snapshot: {} ref(s)", refs.len());
    Ok(refs)
}

/// The absolute private gitdir of `worktree` (`.bare/worktrees/<id>` for a linked
/// worktree), where per-worktree in-progress and config/sparse state lives.
fn worktree_gitdir(worktree: &Path) -> Result<PathBuf> {
    let out = git::output(&["rev-parse", "--absolute-git-dir"], Some(worktree), None)?;
    if !out.status.success() {
        bail!(
            "could not resolve git dir for worktree '{}': {}",
            worktree.display(),
            out.stderr.trim()
        );
    }
    Ok(PathBuf::from(out.stdout.trim()))
}

/// Which mid-operation, if any, is active in `gitdir` (state files git writes
/// while a merge/rebase/cherry-pick/revert/bisect is in flight).
fn in_progress_operation(gitdir: &Path) -> Option<&'static str> {
    if gitdir.join("MERGE_HEAD").exists() {
        return Some("merge");
    }
    if gitdir.join("rebase-merge").is_dir() || gitdir.join("rebase-apply").is_dir() {
        return Some("rebase");
    }
    if gitdir.join("CHERRY_PICK_HEAD").exists() {
        return Some("cherry-pick");
    }
    if gitdir.join("REVERT_HEAD").exists() {
        return Some("revert");
    }
    if gitdir.join("BISECT_START").exists() || gitdir.join("BISECT_LOG").exists() {
        return Some("bisect");
    }
    None
}

/// Per-worktree config or sparse-checkout state in `gitdir` - state a single flat
/// checkout cannot faithfully carry, so its presence blocks the collapse.
fn per_worktree_state(gitdir: &Path) -> Option<&'static str> {
    if gitdir.join("config.worktree").exists() {
        return Some("per-worktree config (config.worktree)");
    }
    if gitdir.join("info").join("sparse-checkout").exists() {
        return Some("a sparse-checkout");
    }
    None
}

/// Whether `ancestor` is an ancestor of (or equal to) `descendant` in
/// `container`'s object graph, via `git merge-base --is-ancestor` (exit 0 = yes).
fn is_ancestor(container: &Path, ancestor: &str, descendant: &str) -> bool {
    git::output(
        &["merge-base", "--is-ancestor", ancestor, descendant],
        Some(container),
        None,
    )
    .map(|o| o.status.success())
    .unwrap_or(false)
}

/// Whether `sha` is reachable from any ref in `container` (i.e. `git for-each-ref
/// --contains <sha>` lists at least one ref). A detached HEAD that is NOT
/// reachable would be orphaned/GC-eligible on collapse.
fn commit_reachable_from_ref(container: &Path, sha: &str) -> bool {
    git::output(
        &["for-each-ref", "--contains", sha, "--format=%(refname)"],
        Some(container),
        None,
    )
    .map(|o| o.status.success() && !o.stdout.trim().is_empty())
    .unwrap_or(false)
}

/// Names of dirty or conflicted submodules in `worktree`, via `git submodule
/// status --recursive` (a `+` prefix = checked-out commit differs; `U` = merge
/// conflict). Uninitialized (`-`) submodules carry no local state to lose, so they
/// are not treated as dirty. A repo with no submodules exits 0 with empty output,
/// so a NON-success exit is a genuine error (undeterminable state) - it bails
/// rather than reporting "clean", so the preflight fails closed on it.
fn dirty_submodules(worktree: &Path) -> Result<Vec<String>> {
    let out = git::output(&["submodule", "status", "--recursive"], Some(worktree), None)?;
    if !out.status.success() {
        bail!(
            "`git submodule status` failed in {}: {}",
            worktree.display(),
            out.stderr.trim()
        );
    }
    let dirty = out
        .stdout
        .lines()
        .filter(|l| l.starts_with('+') || l.starts_with('U'))
        .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
        .collect();
    Ok(dirty)
}

/// Git-ignored paths in `worktree` (`!! ` lines of `status --porcelain
/// --ignored=traditional`), recorded so the summary can report them as archived
/// and recoverable.
fn ignored_files(worktree: &Path) -> Vec<String> {
    git::output(
        &["status", "--porcelain", "--ignored=traditional"],
        Some(worktree),
        None,
    )
    .map(|o| {
        o.stdout
            .lines()
            .filter_map(|l| l.strip_prefix("!! "))
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Warn (operator-facing) about ignored files that will be archived rather than
/// carried into the flat checkout.
fn report_ignored(ignored: &[(PathBuf, Vec<String>)]) {
    for (wt, files) in ignored {
        if !files.is_empty() {
            warn!(
                "flatten: {} git-ignored path(s) in {} will be archived with the container (recover with `rkvr rcvr`): {}",
                files.len(),
                wt.display(),
                files.join(", ")
            );
        }
    }
}

/// Perform the copy-based, crash-safe DB transition (doc steps 1-8). `container`
/// is canonical and confirmed a bare container; `refs_before` is the pre-transition
/// retention baseline. Returns the flat checkout path (== `container`) on success.
fn perform_transition(container: &Path, default: &str, refs_before: &BTreeMap<String, String>) -> Result<PathBuf> {
    debug!("perform_transition: container={:?} default={}", container, default);

    let staging = sibling(container, "flattening")?;
    let backup = sibling(container, "flatten-backup")?;
    let staging_git = staging.join(".git");

    // Clear any leftovers from a failed prior run (recoverable removals).
    common::rkvr::rmrf(&staging)?;
    common::rkvr::rmrf(&backup)?;

    // Step 1: copy the LIVE `.bare/` to `<repo>.flattening/.git/` - never mutate
    // the live container in place.
    fs::create_dir_all(&staging).wrap_err_with(|| format!("creating {}", staging.display()))?;
    copy_dir_all(&container.join(".bare"), &staging_git)
        .wrap_err_with(|| format!("copying .bare into {}", staging_git.display()))?;

    // Step 2: convert the staged config to a non-bare repo. Leave
    // `remote.origin.fetch` untouched (the container already uses the non-bare
    // refspec; introducing `+refs/heads/*:refs/heads/*` could clobber local
    // branches on the next fetch).
    let cfg = staging_git.join("config");
    let cfg_str = cfg.to_string_lossy().into_owned();
    git::run(&["config", "--file", &cfg_str, "core.bare", "false"], None, None)
        .wrap_err("setting core.bare=false in the staged config")?;
    // `--unset` of an absent key exits 5; that is fine - the key just wasn't set.
    let _ = git::output(&["config", "--file", &cfg_str, "--unset", "core.worktree"], None, None);
    git::run(
        &["config", "--file", &cfg_str, "core.logAllRefUpdates", "true"],
        None,
        None,
    )
    .wrap_err("enabling reflogs in the staged config")?;

    // Step 3: remove the staged `.git/worktrees/` admin entries. Safe only because
    // preflight refused all non-discardable per-worktree state; stale entries would
    // otherwise make git think branches are checked out elsewhere.
    common::rkvr::rmrf(&staging_git.join("worktrees"))?;

    // Step 4: materialize the flat tree. Pin HEAD to the default branch, then
    // `reset --hard` with an explicit GIT_DIR/GIT_WORK_TREE so the checkout lands
    // in the staging worktree regardless of cwd.
    let git_dir = staging_git.to_string_lossy().into_owned();
    let work_tree = staging.to_string_lossy().into_owned();
    let envs: [(&str, &str); 2] = [("GIT_DIR", &git_dir), ("GIT_WORK_TREE", &work_tree)];
    git::run(
        &["symbolic-ref", "HEAD", &format!("refs/heads/{}", default)],
        Some(&staging),
        Some(&envs),
    )
    .wrap_err("pointing staged HEAD at the default branch")?;
    git::run(&["reset", "--hard", default], Some(&staging), Some(&envs))
        .wrap_err("materializing the flat working tree")?;

    // Step 5: verify the staged flat checkout before touching the live container.
    verify_flat(&staging, refs_before).wrap_err("staged flat checkout failed verification")?;

    // Step 6: atomic swap, then re-verify at the final path.
    fs::rename(container, &backup).wrap_err_with(|| format!("renaming {} aside", container.display()))?;
    if let Err(e) = fs::rename(&staging, container) {
        // Swap-in failed: restore the original from backup.
        let _ = fs::rename(&backup, container);
        return Err(e).wrap_err("swapping the flattened checkout into place failed");
    }
    if let Err(e) = verify_flat(container, refs_before) {
        // Step 8: post-swap failure - remove the staged final and restore backup.
        let _ = common::rkvr::rmrf(container);
        let _ = fs::rename(&backup, container);
        return Err(e).wrap_err("flattened checkout failed verification after swap");
    }

    // Step 7: only after final verification, remove the backup (recoverably).
    if let Err(e) = common::rkvr::rmrf(&backup) {
        warn!("flatten: could not remove backup {}: {}", backup.display(), e);
    }

    Ok(container.to_path_buf())
}

/// Verify a materialized flat checkout at `dir`: the working tree is clean, every
/// pre-transition ref survives at its exact OID (retention proof), every pre-transition
/// ref OID resolves in the new store (`cat-file -e`), and the object graph is intact
/// (`fsck --connectivity-only`).
fn verify_flat(dir: &Path, refs_before: &BTreeMap<String, String>) -> Result<()> {
    debug!("verify_flat: dir={:?} refs_before={}", dir, refs_before.len());

    let status = git::output(&["status", "--porcelain"], Some(dir), None)?;
    if !status.status.success() {
        bail!("'git status' did not succeed in flattened checkout {}", dir.display());
    }
    if !status.stdout.trim().is_empty() {
        bail!(
            "flattened checkout {} is not clean after materialization:\n{}",
            dir.display(),
            status.stdout.trim()
        );
    }

    // Retention proof: the ref set (name → OID) must be identical before and after.
    let refs_after = refs_snapshot(dir)?;
    if &refs_after != refs_before {
        let mut diffs = Vec::new();
        for (name, oid) in refs_before {
            match refs_after.get(name) {
                None => diffs.push(format!("{} MISSING", name)),
                Some(after) if after != oid => diffs.push(format!("{} CHANGED {} -> {}", name, oid, after)),
                Some(_) => {}
            }
        }
        for name in refs_after.keys() {
            if !refs_before.contains_key(name) {
                diffs.push(format!("{} UNEXPECTED (new)", name));
            }
        }
        bail!(
            "ref retention check failed - refs/* not preserved intact:\n  {}",
            diffs.join("\n  ")
        );
    }

    // Every pre-transition OID must resolve in the staged/final object store.
    for (name, oid) in refs_before {
        let out = git::output(&["cat-file", "-e", oid], Some(dir), None)?;
        if !out.status.success() {
            bail!("ref {} OID {} is missing from the flattened object store", name, oid);
        }
    }

    // Object graph integrity.
    let fsck = git::output(&["fsck", "--connectivity-only"], Some(dir), None)?;
    if !fsck.status.success() {
        bail!(
            "git fsck failed in flattened checkout {}: {}",
            dir.display(),
            fsck.stderr.trim()
        );
    }
    Ok(())
}

/// Recursively copy `src` into `dst`, replicating regular files, directories, and
/// symlinks (git object/ref stores contain no symlinks in practice, but a faithful
/// copy must not follow one out of the tree).
fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).wrap_err_with(|| format!("creating {}", dst.display()))?;
    for entry in fs::read_dir(src).wrap_err_with(|| format!("reading {}", src.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if file_type.is_symlink() {
            let target = fs::read_link(&from)?;
            std::os::unix::fs::symlink(&target, &to)
                .wrap_err_with(|| format!("recreating symlink {}", to.display()))?;
        } else if file_type.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to).wrap_err_with(|| format!("copying {} -> {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// `<parent>/<name>.<suffix>` next to `container`.
fn sibling(container: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = container
        .parent()
        .ok_or_else(|| eyre!("'{}' has no parent directory", container.display()))?;
    let name = container
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| eyre!("'{}' has no file name", container.display()))?;
    Ok(parent.join(format!("{}.{}", name, suffix)))
}

#[cfg(test)]
mod tests;
