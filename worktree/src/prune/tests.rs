use super::*;
use crate::switch::switch;
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

/// A bare container with a `main` worktree, built from a local source (no net).
fn make_container(root: &Path) -> PathBuf {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    git_run(&src, &["init", "-b", "main"]);
    fs::write(src.join("README.md"), "hello").unwrap();
    git_run(&src, &["add", "."]);
    git_run(
        &src,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "init"],
    );

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
    git_run(&container, &["worktree", "add", "main", "main"]);
    container
}

#[test]
fn test_prune_removes_only_merged_clean_worktrees() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // merged: a new branch off main, no extra commits -> ancestor of origin/main.
    let merged = switch(&container, "merged", Some("main")).unwrap();

    // unmerged: a new branch with a commit not on origin/main.
    let wip = switch(&container, "wip", Some("main")).unwrap();
    fs::write(wip.join("new.txt"), "work").unwrap();
    git_run(&wip, &["add", "."]);
    git_run(
        &wip,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "wip"],
    );

    // dirty: merged branch but with uncommitted changes -> protected.
    let dirty = switch(&container, "dirty", Some("main")).unwrap();
    fs::write(dirty.join("scratch.txt"), "uncommitted").unwrap();

    let removed = prune(&container, Some("main"), true).unwrap();

    assert!(removed.contains(&merged), "merged clean worktree should be removed");
    assert!(!removed.contains(&wip), "unmerged worktree must be kept");
    assert!(!removed.contains(&dirty), "dirty worktree must be kept");
    assert!(
        !removed.iter().any(|p| p.ends_with("main")),
        "default worktree must be kept"
    );

    // The checkout is gone but the branch ref survives (the prune invariant).
    assert!(!merged.exists(), "merged worktree dir should be removed");
    let branch_ref = git::output(
        &["show-ref", "--verify", "--quiet", "refs/heads/merged"],
        Some(&container),
        None,
    )
    .unwrap();
    assert!(branch_ref.status.success(), "branch ref 'merged' must survive prune");

    // Kept worktrees still on disk.
    assert!(wip.exists() && dirty.exists());
}

#[test]
fn test_prune_nothing_when_all_unmerged_or_protected() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let wip = switch(&container, "wip", Some("main")).unwrap();
    fs::write(wip.join("x.txt"), "x").unwrap();
    git_run(&wip, &["add", "."]);
    git_run(
        &wip,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "x"],
    );

    let removed = prune(&container, Some("main"), true).unwrap();
    assert!(removed.is_empty(), "nothing merged -> nothing removed; got {removed:?}");
}

#[test]
fn test_prune_non_interactive_without_yes_bails() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());
    switch(&container, "merged", Some("main")).unwrap();

    // assume_yes=false in the (non-tty) test context must refuse rather than
    // remove without confirmation.
    let err = prune(&container, Some("main"), false).unwrap_err();
    assert!(
        format!("{err}").contains("--yes"),
        "non-interactive prune without --yes should bail; got: {err}"
    );
}
