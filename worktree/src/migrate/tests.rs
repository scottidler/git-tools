use super::*;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

/// Test remover: never invokes `rkvr`. `require` always succeeds; `rmrf` removes
/// via plain `std::fs` (no-op on a missing path). Keeps migrate tests from shelling
/// out to the real `rkvr` binary or touching its archive.
struct FsRemover;

impl common::rkvr::Remover for FsRemover {
    fn require(&self) -> eyre::Result<()> {
        Ok(())
    }
    fn rmrf(&self, path: &Path) -> eyre::Result<()> {
        match path.symlink_metadata() {
            Err(_) => Ok(()),
            Ok(meta) if meta.is_dir() => Ok(fs::remove_dir_all(path)?),
            Ok(_) => Ok(fs::remove_file(path)?),
        }
    }
}

/// The `wip/*` rescue branches present in a migrated container.
fn wip_branches(container: &Path) -> Vec<String> {
    let out = git::output(
        &["branch", "--list", "wip/*", "--format=%(refname:short)"],
        Some(container),
        None,
    )
    .unwrap();
    out.stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Whether any `wip/*` branch carries `file` with the given `content`.
fn any_wip_has(container: &Path, branches: &[String], file: &str, content: &str) -> bool {
    branches.iter().any(|b| {
        let r = git::output(&["show", &format!("{}:{}", b, file)], Some(container), None).unwrap();
        r.status.success() && r.stdout.trim() == content
    })
}

fn git_run(dir: &Path, args: &[&str]) {
    let out = git::output(args, Some(dir), None).unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        out.stderr
    );
}

fn commit(dir: &Path, msg: &str) {
    git_run(
        dir,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", msg],
    );
}

/// A bare "remote" with one commit on `main`, returned as a path usable as an
/// origin URL.
fn make_remote(root: &Path) -> PathBuf {
    let seed = root.join("seed");
    fs::create_dir_all(&seed).unwrap();
    git_run(&seed, &["init", "-b", "main"]);
    fs::write(seed.join("README.md"), "hello").unwrap();
    git_run(&seed, &["add", "."]);
    commit(&seed, "init");

    let remote = root.join("remote.git");
    git_run(
        root,
        &["clone", "--bare", seed.to_str().unwrap(), remote.to_str().unwrap()],
    );
    remote
}

/// A flat checkout of `remote` at `<root>/work/org/repo`.
fn make_flat(root: &Path, remote: &Path) -> PathBuf {
    let flat = root.join("work").join("org").join("repo");
    fs::create_dir_all(flat.parent().unwrap()).unwrap();
    git_run(root, &["clone", remote.to_str().unwrap(), flat.to_str().unwrap()]);
    flat
}

#[test]
fn test_migrate_clean_flat_to_bare() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();

    // The flat path is now a bare container with a default-branch worktree.
    assert!(bare::is_bare_container(&flat), "should be a bare container");
    assert_eq!(worktree, flat.join("main"));
    assert!(worktree.join("README.md").is_file(), "worktree should be checked out");

    // The worktree links must be functional after the swap.
    let out = git::output(&["status", "--porcelain"], Some(&worktree), None).unwrap();
    assert!(out.status.success(), "git status should work in the migrated worktree");

    // Origin is the real remote, not the local path it was bare-cloned from.
    let origin = git::output(&["remote", "get-url", "origin"], Some(&worktree), None).unwrap();
    assert_eq!(origin.stdout.trim(), remote.to_str().unwrap());

    // No staging/backup dirs left behind.
    assert!(!root.join("work").join("org").join("repo.migrating").exists());
    assert!(!root.join("work").join("org").join("repo.backup").exists());
}

// NOTE: these two tests previously asserted that migrate REFUSED a dirty tree /
// non-empty stash. As of the rescue pass, migrate auto-rescues both into wip/*
// branches and succeeds, so they now assert successful rescue + content
// recovery. (Converted in Phase 2 rather than Phase 4 to keep otto ci green
// every phase - see implementation notes.)

#[test]
fn test_migrate_rescues_dirty_tree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Uncommitted change in the main worktree.
    fs::write(flat.join("README.md"), "dirty-xyz").unwrap();

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    assert!(bare::is_bare_container(&flat), "migration should succeed");
    let wips = wip_branches(container);
    assert!(!wips.is_empty(), "expected a wip/* rescue branch");
    assert!(
        any_wip_has(container, &wips, "README.md", "dirty-xyz"),
        "the dirty content must be recoverable from a wip/* branch; wips={wips:?}"
    );
}

#[test]
fn test_migrate_rescues_nonempty_stash() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A stash entry, leaving the tree clean.
    fs::write(flat.join("README.md"), "stashed-xyz").unwrap();
    git_run(&flat, &["stash", "push", "-m", "wip"]);

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    assert!(bare::is_bare_container(&flat), "migration should succeed");
    let wips = wip_branches(container);
    assert!(
        any_wip_has(container, &wips, "README.md", "stashed-xyz"),
        "the stashed content must be recoverable from a wip/* branch; wips={wips:?}"
    );
}

#[test]
fn test_migrate_rescues_dirty_linked_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A linked worktree with an uncommitted change.
    let linked = root.join("linked");
    git_run(&flat, &["worktree", "add", "-b", "side", linked.to_str().unwrap()]);
    fs::write(linked.join("README.md"), "linked-dirty").unwrap();

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    let wips = wip_branches(container);
    assert!(
        any_wip_has(container, &wips, "README.md", "linked-dirty"),
        "a dirty LINKED worktree must be rescued, not silently stranded; wips={wips:?}"
    );
}

#[test]
fn test_migrate_rescues_detached_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A detached worktree carrying a UNIQUE commit (no branch points at it).
    let det = root.join("det");
    git_run(&flat, &["worktree", "add", "--detach", det.to_str().unwrap()]);
    fs::write(det.join("UNIQUE.txt"), "detached-work").unwrap();
    git_run(&det, &["add", "."]);
    commit(&det, "detached unique commit");
    let dsha = {
        let out = git::output(&["rev-parse", "HEAD"], Some(&det), None).unwrap();
        out.stdout.trim().to_string()
    };

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    let wips = wip_branches(container);
    assert!(
        wips.iter().any(|w| w.starts_with("wip/detached-")),
        "detached worktree must get a wip/detached-* branch; wips={wips:?}"
    );
    // The unique commit is reachable (not a dangling, GC-eligible object).
    let reachable = git::output(&["cat-file", "-e", &dsha], Some(container), None).unwrap();
    assert!(
        reachable.status.success(),
        "detached unique commit must be reachable in the container"
    );
    assert!(
        any_wip_has(container, &wips, "UNIQUE.txt", "detached-work"),
        "detached commit content must be recoverable; wips={wips:?}"
    );
}

#[test]
fn test_migrate_bails_on_unmerged_tree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Force a merge conflict so the tree has unmerged paths.
    fs::write(flat.join("F"), "base\n").unwrap();
    git_run(&flat, &["add", "."]);
    commit(&flat, "base");
    git_run(&flat, &["checkout", "-b", "x"]);
    fs::write(flat.join("F"), "x\n").unwrap();
    git_run(&flat, &["add", "."]);
    commit(&flat, "x");
    git_run(&flat, &["checkout", "main"]);
    fs::write(flat.join("F"), "main\n").unwrap();
    git_run(&flat, &["add", "."]);
    commit(&flat, "main");
    // --no-ff forces the 3-way merge attempt regardless of a `merge.ff=only`
    // git config, so the conflict (and unmerged paths) actually materialize.
    let merge = git::output(
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "merge", "--no-ff", "x"],
        Some(&flat),
        None,
    )
    .unwrap();
    assert!(!merge.status.success(), "merge should conflict (test setup)");

    let err = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap_err();
    assert!(
        format!("{err}").contains("unmerged"),
        "should bail on unmerged paths; got: {err}"
    );
    assert!(!bare::is_bare_container(&flat), "must not migrate a mid-merge repo");
}

#[test]
fn test_wip_branch_name_truncates_long_slug() {
    let mut used: HashSet<String> = HashSet::new();
    let long = "a".repeat(300);
    let name = wip_branch_name(&long, &mut used);
    assert!(name.starts_with("wip/"));
    assert!(
        name.len() <= "wip/".len() + WIP_SLUG_MAX,
        "wip name must be length-capped: {name}"
    );
}

#[test]
fn test_wip_branch_name_prefix_aware() {
    // Exact collision with an existing branch -> suffixed.
    let mut used: HashSet<String> = ["wip/foo".to_string()].into_iter().collect();
    assert_ne!(wip_branch_name("foo", &mut used), "wip/foo");

    // Directory/file path-prefix conflict (existing wip/bar/baz blocks wip/bar).
    let mut used2: HashSet<String> = ["wip/bar/baz".to_string()].into_iter().collect();
    assert_ne!(wip_branch_name("bar", &mut used2), "wip/bar");
}

#[test]
fn test_migrate_preserves_clean_but_ahead_commits() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A committed-but-unpushed commit on main (clean tree, ahead of origin).
    fs::write(flat.join("ahead.txt"), "local work").unwrap();
    git_run(&flat, &["add", "."]);
    commit(&flat, "unpushed");
    let ahead_sha = {
        let out = git::output(&["rev-parse", "HEAD"], Some(&flat), None).unwrap();
        out.stdout.trim().to_string()
    };

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();

    // The unpushed commit must survive into the migrated worktree.
    let head = git::output(&["rev-parse", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(head.stdout.trim(), ahead_sha, "unpushed commit must survive");
    assert!(worktree.join("ahead.txt").is_file());
}

#[test]
fn test_migrate_from_non_default_branch_creates_default_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Check out a NON-default branch with an unpushed commit and leave it active.
    git_run(&flat, &["checkout", "-b", "feature"]);
    fs::write(flat.join("f.txt"), "feature work").unwrap();
    git_run(&flat, &["add", "."]);
    commit(&flat, "feature commit");

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();

    // The canonical worktree must be the TRUE default (main), not the
    // checked-out feature branch (the Finding 1 bug).
    assert_eq!(worktree, flat.join("main"), "default worktree must be main");
    assert!(flat.join("main").is_dir(), "default-branch worktree must be present");
    assert!(
        flat.join("feature").is_dir(),
        "previously checked-out branch must also get a worktree"
    );

    // Container HEAD must be reset to the default so the cd/z shim + discovery
    // resolve the right canonical worktree.
    let head = git::output(&["symbolic-ref", "--short", "HEAD"], Some(&flat), None).unwrap();
    assert_eq!(
        head.stdout.trim(),
        "main",
        "container HEAD must point to the default branch"
    );

    // The feature branch's unpushed commit survived.
    let log = git::output(&["log", "--oneline", "-1", "feature"], Some(&flat), None).unwrap();
    assert!(
        log.stdout.contains("feature commit"),
        "feature's unpushed commit must survive; got: {:?}",
        log.stdout
    );
}

// `flat_from_dir` is tested instead of `flat_from_cwd` so these don't mutate the
// process-global cwd (which would race with parallel tests).

#[test]
fn test_flat_from_dir_resolves_main_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);
    let main = flat.canonicalize().unwrap();

    let linked = root.join("linked");
    git_run(&flat, &["worktree", "add", "-b", "feature", linked.to_str().unwrap()]);
    let sub = flat.join("a").join("b");
    fs::create_dir_all(&sub).unwrap();

    assert_eq!(
        flat_from_dir(&sub).unwrap().canonicalize().unwrap(),
        main,
        "from a subdirectory, resolves the main worktree"
    );
    assert_eq!(
        flat_from_dir(&linked).unwrap().canonicalize().unwrap(),
        main,
        "from a linked worktree, resolves the main worktree"
    );
}

#[test]
fn test_flat_from_dir_rejects_bare_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);
    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();

    let err = flat_from_dir(&worktree).unwrap_err();
    assert!(
        format!("{err}").contains("already a bare container"),
        "should reject an already-migrated layout; got: {err}"
    );
}

/// Rough-edge #1 regression: the target path is absolutized (canonicalized)
/// before staging, so the post-swap `git worktree repair` always agrees with its
/// cwd. Passing a non-canonical input (here a symlink) with two worktrees
/// (default + a non-default checked-out branch) must still yield a container
/// rooted at the REAL path with every worktree repaired - the inconsistency that
/// broke repair before the fix. (Symlink, not a relative path, so the test never
/// mutates the process-global cwd and so cannot race with parallel tests.)
#[test]
fn test_migrate_canonicalizes_target_path() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let remote = make_remote(&root);
    let flat = make_flat(&root, &remote);
    git_run(&flat, &["checkout", "-b", "feature"]);

    let link = root.join("link-to-repo");
    std::os::unix::fs::symlink(&flat, &link).unwrap();

    let worktree = migrate_flat_to_bare(&link, Some("main"), &FsRemover).unwrap();

    // The container resolves to the REAL path, not the symlink input.
    assert_eq!(
        worktree,
        flat.join("main"),
        "worktree must be rooted at the canonical path"
    );
    assert!(worktree.join("README.md").is_file());
    let feature = flat.join("feature");
    assert!(feature.is_dir(), "the non-default worktree must be present");
    let out = git::output(&["status", "--porcelain"], Some(&feature), None).unwrap();
    assert!(out.status.success(), "feature worktree must be functional after repair");
}

#[test]
fn test_migrate_carries_linked_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A clean linked worktree on its own branch.
    let linked = root.join("repo-side");
    git_run(&flat, &["worktree", "add", "-b", "side", linked.to_str().unwrap()]);

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    // The linked branch is recreated as a native worktree inside the container.
    let side = container.join("side");
    assert!(
        side.is_dir(),
        "linked worktree 'side' must be carried into the container"
    );
    let out = git::output(&["status", "--porcelain"], Some(&side), None).unwrap();
    assert!(out.status.success(), "carried worktree must be functional");

    // The old external orphan dir is removed (recoverably).
    assert!(!linked.exists(), "orphaned external worktree dir must be removed");
}

/// Slug-unification (Phase 3): a linked worktree on a SLASHED branch is recreated
/// at the slugified dir (`feature/auth` -> `feature-auth`), matching what the
/// `worktree` tool produces - not the raw nested `feature/auth` path the old
/// `clone --migrate` would have created.
#[test]
fn test_migrate_slugifies_slashed_linked_worktree_dir() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A clean linked worktree on a slashed branch.
    let linked = root.join("repo-auth");
    git_run(
        &flat,
        &["worktree", "add", "-b", "feature/auth", linked.to_str().unwrap()],
    );

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    // The carried worktree lands at the slug dir, not the raw nested path.
    assert!(
        container.join("feature-auth").is_dir(),
        "slashed branch must be carried at the slugified dir 'feature-auth'"
    );
    assert!(
        !container.join("feature").join("auth").exists(),
        "the raw nested 'feature/auth' path must NOT be created"
    );
    let out = git::output(&["status", "--porcelain"], Some(&container.join("feature-auth")), None).unwrap();
    assert!(out.status.success(), "slugged worktree must be functional");
}

#[test]
fn test_migrate_slugifies_slashed_default_worktree_dir() {
    // Regression (review-panel audit): when the REMOTE default branch is slashed
    // (e.g. `release/1.2`), the default worktree lands at the slug dir
    // `release-1-2`, but migrate used to verify and swap against the raw nested
    // path `release/1.2`. That path doesn't exist, so verify bailed and the whole
    // migration rolled back. The default dir must be slugified everywhere; the
    // migration must commit.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // A remote whose default branch is the slashed `release/1.2`.
    let seed = root.join("seed");
    fs::create_dir_all(&seed).unwrap();
    git_run(&seed, &["init", "-b", "release/1.2"]);
    fs::write(seed.join("README.md"), "hello").unwrap();
    git_run(&seed, &["add", "."]);
    commit(&seed, "init");
    let remote = root.join("remote.git");
    git_run(
        root,
        &["clone", "--bare", seed.to_str().unwrap(), remote.to_str().unwrap()],
    );
    git_run(&remote, &["symbolic-ref", "HEAD", "refs/heads/release/1.2"]);

    let flat = root.join("work").join("org").join("repo");
    fs::create_dir_all(flat.parent().unwrap()).unwrap();
    git_run(root, &["clone", remote.to_str().unwrap(), flat.to_str().unwrap()]);

    let worktree = migrate_flat_to_bare(&flat, Some("release/1.2"), &FsRemover).unwrap();

    // Migration committed to a bare container, default worktree at the SLUG dir.
    assert!(
        bare::is_bare_container(&flat),
        "migration must have committed (no rollback)"
    );
    assert_eq!(
        worktree,
        flat.join("release-1-2"),
        "default worktree dir must be slugified"
    );
    assert!(
        flat.join("release-1-2").is_dir(),
        "slugified default worktree must exist"
    );
    assert!(
        !flat.join("release").exists(),
        "the raw nested 'release/1.2' default path must NOT be created"
    );
    let out = git::output(&["status", "--porcelain"], Some(&worktree), None).unwrap();
    assert!(out.status.success(), "slugged default worktree must be functional");
}

#[test]
fn test_migrate_skips_linked_worktree_on_default_branch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Main worktree moves to a feature branch; a linked worktree sits on the
    // DEFAULT branch (main). Recreating it would be a fatal double-checkout, so
    // it must be skipped - migration must still succeed.
    git_run(&flat, &["checkout", "-b", "feature"]);
    let linked = root.join("repo-main");
    git_run(&flat, &["worktree", "add", linked.to_str().unwrap(), "main"]);

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    assert_eq!(worktree, flat.join("main"), "default worktree must be main");
    assert!(container.join("main").is_dir(), "default worktree present");
    assert!(container.join("feature").is_dir(), "current (feature) worktree present");
    assert!(
        !linked.exists(),
        "orphaned external dir for the default-branch worktree must be removed"
    );
}

#[test]
fn test_migrate_detects_ignored_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // An ignored file (git-ignored -> the tree is still "clean" to git).
    fs::write(flat.join(".gitignore"), ".env\n").unwrap();
    git_run(&flat, &["add", ".gitignore"]);
    commit(&flat, "add gitignore");
    fs::write(flat.join(".env"), "SECRET=1").unwrap();

    let ignored = ignored_files(&flat);
    assert!(
        ignored.iter().any(|p| p.contains(".env")),
        "the ignored .env must be detected for the summary; got {ignored:?}"
    );

    // Ignored files do not block migration (they aren't dirty to git).
    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    assert!(worktree.join("README.md").is_file());
}

#[test]
fn test_dry_run_makes_no_changes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A dirty tree + a linked worktree, to exercise the plan paths.
    fs::write(flat.join("README.md"), "dirty").unwrap();
    let linked = root.join("repo-side");
    git_run(&flat, &["worktree", "add", "-b", "side", linked.to_str().unwrap()]);

    let result = dry_run(&flat, Some("main"), &FsRemover).unwrap();

    // Returns the canonical flat path and changed NOTHING.
    assert_eq!(result, flat.canonicalize().unwrap());
    assert!(!bare::is_bare_container(&flat), "dry-run must not migrate");
    assert!(flat.join(".git").is_dir(), "still a flat checkout");
    assert_eq!(
        fs::read_to_string(flat.join("README.md")).unwrap(),
        "dirty",
        "dirty change untouched"
    );
    assert!(linked.exists(), "linked worktree dir must remain");
    let wip = git::output(&["branch", "--list", "wip/*"], Some(&flat), None).unwrap();
    assert!(wip.stdout.trim().is_empty(), "dry-run must not create wip/* branches");
    assert!(!root.join("work").join("org").join("repo.migrating").exists());
    assert!(!root.join("work").join("org").join("repo.backup").exists());
}

#[test]
fn test_target_symlink_detected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let dest = root.join("real-target");
    fs::create_dir_all(&dest).unwrap();
    let flat = root.join("repo");
    fs::create_dir_all(&flat).unwrap();
    std::os::unix::fs::symlink(&dest, flat.join("target")).unwrap();

    assert_eq!(target_symlink(&flat).unwrap(), dest);
    assert!(target_symlink(&root.join("no-such-repo")).is_none());
}

#[test]
fn test_migrate_preserves_local_only_branch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A local-only branch that never existed on origin.
    git_run(&flat, &["branch", "local-only"]);

    let worktree = migrate_flat_to_bare(&flat, Some("main"), &FsRemover).unwrap();
    let container = worktree.parent().unwrap();

    // The local-only branch must survive in the migrated bare database.
    let out = git::output(&["branch", "--list", "local-only"], Some(container), None).unwrap();
    assert!(
        out.stdout.contains("local-only"),
        "local-only branch must survive migration; got: {:?}",
        out.stdout
    );
}
