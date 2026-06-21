use super::*;
use std::fs;
use tempfile::TempDir;

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

#[test]
fn test_migrate_refuses_dirty_tree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Uncommitted change.
    fs::write(flat.join("README.md"), "dirty").unwrap();

    let err = migrate_flat_to_bare(&flat, Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("uncommitted or untracked"),
        "should refuse a dirty tree; got: {err}"
    );
    // Original untouched.
    assert!(!bare::is_bare_container(&flat));
}

#[test]
fn test_migrate_refuses_nonempty_stash() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let remote = make_remote(root);
    let flat = make_flat(root, &remote);

    // Create a stash entry, leaving the tree clean.
    fs::write(flat.join("README.md"), "wip").unwrap();
    git_run(&flat, &["stash", "push", "-m", "wip"]);

    let err = migrate_flat_to_bare(&flat, Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("stash is non-empty"),
        "should refuse a non-empty stash; got: {err}"
    );
    assert!(!bare::is_bare_container(&flat));
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
