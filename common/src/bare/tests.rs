use super::*;
use std::fs;
use std::path::Path;
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
/// container path. No network - `git clone --bare` from a local source.
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
fn test_resolve_new_branch_slugifies_branch_and_dir() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let worktree = resolve_and_add(&container, "Add Auth", Some("main")).unwrap();

    // New branch: slug used as both branch and dir.
    assert_eq!(worktree, container.join("add-auth"));
    assert!(worktree.join("README.md").is_file(), "worktree should be checked out");

    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "add-auth", "new branch should be the slug");
}

#[test]
fn test_resolve_existing_branch_keeps_real_name() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // feature/auth came with the bare clone as a local head; keep its real
    // slashed name, slugify only the directory.
    let worktree = resolve_and_add(&container, "feature/auth", Some("main")).unwrap();

    assert_eq!(worktree, container.join("feature-auth"), "dir is slugified");
    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "feature/auth", "branch keeps its real slashed name");
}

#[test]
fn test_resolve_remote_only_branch_tracks_upstream() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // A branch that appears on origin AFTER the container was built is
    // remote-only (no local head), so resolve creates a tracking local branch.
    git_run(&tmp.path().join("src"), &["branch", "hotfix"]);
    git_run(&container, &["fetch", "origin"]);

    let worktree = resolve_and_add(&container, "hotfix", Some("main")).unwrap();
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
fn test_slug_collision_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // `feature/auth` checks out into dir `feature-auth`.
    resolve_and_add(&container, "feature/auth", Some("main")).unwrap();

    // A literal new branch `feature-auth` slugs to the SAME dir; the branch is
    // not checked out anywhere, but the derived dir is occupied by an unrelated
    // tree, so ReuseOrBail must bail rather than silently reuse it.
    let err = resolve_and_add(&container, "feature-auth", Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("slug collision"),
        "colliding dir should be rejected, not silently reused; got: {err}"
    );
}

#[test]
fn test_resolve_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let first = resolve_and_add(&container, "topic", Some("main")).unwrap();
    // Re-running for the same branch just returns the existing worktree.
    let second = resolve_and_add(&container, "topic", Some("main")).unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_resolve_empty_slug_is_rejected() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    let err = resolve_and_add(&container, "///", Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("slugifies to empty"),
        "a name with no alphanumerics should be rejected; got: {err}"
    );
}

#[test]
fn test_add_worktree_empty_slug_rejected_for_existing_branch() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // `slugify_branch` can return empty for ANY source, not just NewFrom. An
    // existing/remote branch whose name has no alphanumerics must be rejected by
    // the primitive itself, before any `git worktree add`.
    let err = add_worktree(
        &container,
        &AddSpec {
            branch: "///",
            source: Source::ExistingLocal,
            collision: Collision::ReuseOrBail,
        },
    )
    .unwrap_err();
    assert!(
        format!("{err}").contains("slugifies to empty"),
        "an existing-branch name with no alphanumerics should be rejected; got: {err}"
    );
}

#[test]
fn test_reuse_finds_legacy_raw_path_by_branch() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // Simulate a worktree created by OLD `clone` at the RAW pre-slug path: the
    // slashed branch `feature/auth` sits at `container/feature/auth`, not at the
    // newly-derived slug dir `container/feature-auth`.
    git_run(&container, &["worktree", "add", "feature/auth", "feature/auth"]);
    let raw_path = container.join("feature").join("auth");
    assert!(raw_path.join(".git").is_file(), "legacy raw-path worktree should exist");

    // ReuseOrBail must locate the branch via `git worktree list` (by branch, not
    // by derived dir) and return the existing raw path, never attempt a second
    // `git worktree add` (git rejects an already-checked-out branch fatally).
    let reused = add_worktree(
        &container,
        &AddSpec {
            branch: "feature/auth",
            source: Source::ExistingLocal,
            collision: Collision::ReuseOrBail,
        },
    )
    .unwrap();
    assert_eq!(reused, raw_path, "reuse the legacy raw path, not the derived slug dir");
    assert!(
        !container.join("feature-auth").exists(),
        "must not have created a second checkout at the slug dir"
    );
}

#[test]
fn test_resolve_worktrees_bare_entry_only() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // No worktree has been added yet: the only row is the bare entry itself.
    let rows = resolve_worktrees(&container).unwrap();
    assert_eq!(rows.len(), 1);
    assert!(rows[0].bare);
    assert_eq!(rows[0].branch, None);
    assert_eq!(rows[0].head, None, "the bare entry carries no HEAD line");
    assert!(!rows[0].locked);
}

#[test]
fn test_resolve_worktrees_reads_branch_detached_and_locked() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    git_run(&container, &["worktree", "add", "main", "main"]);
    git_run(&container, &["worktree", "add", "--detach", "detached"]);
    git_run(&container, &["worktree", "lock", "detached", "--reason", "testing"]);

    let rows = resolve_worktrees(&container).unwrap();
    assert_eq!(rows.len(), 3, "bare entry + main + detached");

    let bare = rows.iter().find(|r| r.bare).expect("bare row present");
    assert_eq!(bare.path, container.join(".bare"));

    let main = rows
        .iter()
        .find(|r| r.branch.as_deref() == Some("main"))
        .expect("main worktree present");
    assert_eq!(main.path, container.join("main"));
    assert!(main.head.is_some(), "checked-out worktree carries a HEAD sha");
    assert!(!main.locked);

    let detached = rows
        .iter()
        .find(|r| !r.bare && r.branch.is_none())
        .expect("detached worktree present");
    assert_eq!(detached.path, container.join("detached"));
    assert!(detached.head.is_some(), "detached worktree still carries a HEAD sha");
    assert!(detached.locked, "the `locked <reason>` line must set locked");
}

#[test]
fn test_uniquify_appends_suffix_on_collision() {
    let tmp = TempDir::new().unwrap();
    let container = make_container(tmp.path());

    // First add lands at the bare slug dir.
    let first = add_worktree(
        &container,
        &AddSpec {
            branch: "feature/auth",
            source: Source::ExistingLocal,
            collision: Collision::Uniquify,
        },
    )
    .unwrap();
    assert_eq!(first, container.join("feature-auth"));

    // A literal `feature-auth` slugs to the same dir; Uniquify appends `-1`
    // (probed via Path::exists) and returns the suffixed path.
    git_run(&tmp.path().join("src"), &["branch", "feature-auth"]);
    git_run(&container, &["fetch", "origin"]);
    let second = add_worktree(
        &container,
        &AddSpec {
            branch: "feature-auth",
            source: Source::ExistingLocal,
            collision: Collision::Uniquify,
        },
    )
    .unwrap();
    assert_eq!(second, container.join("feature-auth-1"), "suffix appended on collision");
}
