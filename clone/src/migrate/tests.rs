use super::*;
use std::collections::HashSet;
use std::fs;
use tempfile::TempDir;

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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();

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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();
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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();
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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();
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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();
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
    let merge = git::output(&["merge", "--no-ff", "x"], Some(&flat), None).unwrap();
    assert!(!merge.status.success(), "merge should conflict (test setup)");

    let err = migrate_flat_to_bare(&flat, Some("main")).unwrap_err();
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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();

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

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();

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
    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();

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

    let worktree = migrate_flat_to_bare(&link, Some("main")).unwrap();

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
fn test_migrate_preserves_local_only_branch() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // A local-only branch that never existed on origin.
    git_run(&flat, &["branch", "local-only"]);

    let worktree = migrate_flat_to_bare(&flat, Some("main")).unwrap();
    let container = worktree.parent().unwrap();

    // The local-only branch must survive in the migrated bare database.
    let out = git::output(&["branch", "--list", "local-only"], Some(container), None).unwrap();
    assert!(
        out.stdout.contains("local-only"),
        "local-only branch must survive migration; got: {:?}",
        out.stdout
    );
}
