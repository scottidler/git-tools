use super::*;
use crate::bare;
use crate::config::{Layout, Op};
use common::git::RepoSpec;
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

/// Local source repo with `main` plus an extra remote branch `feature/auth`.
fn make_source(root: &Path) {
    let src = root.join("origin").join("org").join("repo");
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
}

/// Set up a bare container at `<root>/work/org/repo` and return both the
/// `Config` (for worktree::add) and the container path.
fn setup(root: &Path) -> (Config, PathBuf) {
    make_source(root);
    let config = Config {
        spec: Some(RepoSpec {
            org: "org".to_string(),
            repo: "repo".to_string(),
        }),
        op: Op::Clone,
        layout: Layout::Bare,
        revision: "HEAD".to_string(),
        remote: root.join("origin").to_string_lossy().into_owned(),
        clonepath: root.join("work"),
        mirrorpath: None,
        versioning: false,
        verbose: false,
        ssh_key: None,
        default_branch: Some("main".to_string()),
    };
    let spec = config.spec.clone().unwrap();
    bare::setup_bare_container(&config, &spec).unwrap();
    let container = root.join("work").join("org").join("repo");
    (config, container)
}

/// `config` rewritten to an AddWorktree op for `branch`.
fn worktree_config(base: &Config, branch: &str) -> Config {
    Config {
        spec: base.spec.clone(),
        op: Op::AddWorktree(branch.to_string()),
        layout: base.layout,
        revision: base.revision.clone(),
        remote: base.remote.clone(),
        clonepath: base.clonepath.clone(),
        mirrorpath: base.mirrorpath.clone(),
        versioning: base.versioning,
        verbose: base.verbose,
        ssh_key: base.ssh_key.clone(),
        default_branch: base.default_branch.clone(),
    }
}

#[test]
fn test_add_new_branch_slugifies_branch_and_dir() {
    let tmp = TempDir::new().unwrap();
    let (base, container) = setup(tmp.path());

    let config = worktree_config(&base, "Add Auth");
    let worktree = add(&config, "Add Auth").unwrap();

    // New branch: slug used as both branch and dir.
    assert_eq!(worktree, container.join("add-auth"));
    assert!(worktree.join("README.md").is_file(), "worktree should be checked out");

    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "add-auth", "new branch should be the slug");
}

#[test]
fn test_add_existing_remote_branch_keeps_real_name() {
    let tmp = TempDir::new().unwrap();
    let (base, container) = setup(tmp.path());

    // feature/auth exists on origin; keep its real name, slugify only the dir.
    let config = worktree_config(&base, "feature/auth");
    let worktree = add(&config, "feature/auth").unwrap();

    assert_eq!(worktree, container.join("feature-auth"), "dir is slugified");
    let out = git::output(&["rev-parse", "--abbrev-ref", "HEAD"], Some(&worktree), None).unwrap();
    assert_eq!(out.stdout.trim(), "feature/auth", "branch keeps its real slashed name");
}

#[test]
fn test_add_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let (base, _container) = setup(tmp.path());

    let config = worktree_config(&base, "topic");
    let first = add(&config, "topic").unwrap();
    // Re-running for the same branch just returns the existing worktree.
    let second = add(&config, "topic").unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_add_rejects_non_bare_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    // A flat checkout, not a bare container.
    let flat = root.join("work").join("org").join("repo");
    fs::create_dir_all(&flat).unwrap();
    fs::create_dir_all(flat.join(".git")).unwrap();

    let config = Config {
        spec: Some(RepoSpec {
            org: "org".to_string(),
            repo: "repo".to_string(),
        }),
        op: Op::AddWorktree("feat".to_string()),
        layout: Layout::Bare,
        revision: "HEAD".to_string(),
        remote: "ssh://git@github.com".to_string(),
        clonepath: root.join("work"),
        mirrorpath: None,
        versioning: false,
        verbose: false,
        ssh_key: None,
        default_branch: None,
    };

    let err = add(&config, "feat").unwrap_err();
    assert!(
        format!("{err}").contains("not a bare container"),
        "should reject a flat checkout; got: {err}"
    );
}
