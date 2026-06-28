use super::*;
use std::fs;

use common::git;
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

/// Build a real bare container at `<root>/repo` from a local source repo that
/// has `main` plus a slashed remote branch `feature/auth`. Returns the
/// container path. No network — `git clone --bare` from a local source.
pub(crate) fn make_container(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    git_run(&src, &["init", "-b", "main"]);
    fs::write(src.join("README.md"), "hello").unwrap();
    git_run(&src, &["add", "."]);
    git_run(
        &src,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "init"],
    );
    // A colleague's slashed branch we must not rename.
    git_run(&src, &["branch", "feature/auth"]);

    let container = root.join("repo");
    fs::create_dir_all(&container).unwrap();
    let bare = container.join(".bare");
    git_run(
        root,
        &["clone", "--bare", src.to_str().unwrap(), bare.to_str().unwrap()],
    );
    fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
    git_run(
        &container,
        &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"],
    );
    git_run(&container, &["fetch", "origin"]);
    container
}

#[test]
fn test_switch_new_branch_slugifies_branch_and_dir() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let worktree = switch(&container, "Add Auth", Some("main")).unwrap();

    // New branch: slug used as both branch and dir.
    assert_eq!(worktree, container.join("add-auth"));
    assert!(worktree.join("README.md").is_file(), "worktree should be checked out");

    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "add-auth", "new branch should be the slug");
}

#[test]
fn test_switch_existing_branch_keeps_real_name() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // feature/auth came with the bare clone as a local head; keep its real
    // slashed name, slugify only the directory.
    let worktree = switch(&container, "feature/auth", Some("main")).unwrap();

    assert_eq!(worktree, container.join("feature-auth"), "dir is slugified");
    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "feature/auth", "branch keeps its real slashed name");
}

#[test]
fn test_switch_remote_only_branch_tracks_upstream() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // A branch that appears on origin AFTER the container was built is
    // remote-only (no local head), so switch creates a tracking local branch.
    git_run(&tmp.path().join("src"), &["branch", "hotfix"]);
    git_run(&container, &["fetch", "origin"]);

    let worktree = switch(&container, "hotfix", Some("main")).unwrap();
    assert_eq!(worktree, container.join("hotfix"));

    let upstream = git::output(
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        Some(&worktree),
        None,
    )
    .unwrap();
    assert_eq!(
        upstream.stdout.trim(),
        "origin/hotfix",
        "remote-only branch should track upstream"
    );
}

#[test]
fn test_switch_slug_collision_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // `feature/auth` checks out into dir `feature-auth`.
    switch(&container, "feature/auth", Some("main")).unwrap();

    // A literal new branch `feature-auth` slugs to the SAME dir; rather than
    // silently reuse the `feature/auth` tree, switch must bail.
    let err = switch(&container, "feature-auth", Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("slug collision"),
        "colliding dir should be rejected, not silently reused; got: {err}"
    );
}

#[test]
fn test_switch_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let first = switch(&container, "topic", Some("main")).unwrap();
    // Re-running for the same branch just returns the existing worktree.
    let second = switch(&container, "topic", Some("main")).unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_switch_empty_slug_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let err = switch(&container, "///", Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("slugifies to empty"),
        "a name with no alphanumerics should be rejected; got: {err}"
    );
}
