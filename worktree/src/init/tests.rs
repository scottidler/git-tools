use super::*;
use crate::config::Op;
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

/// A local source repo with one commit on `main` at `<root>/origin/<org>/<repo>`.
fn make_source(root: &Path, org: &str, repo: &str) {
    let src = root.join("origin").join(org).join(repo);
    fs::create_dir_all(&src).unwrap();
    git_run(&src, &["init", "-b", "main"]);
    fs::write(src.join("README.md"), "hello").unwrap();
    git_run(&src, &["add", "."]);
    git_run(
        &src,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "init"],
    );
}

fn spec(org: &str, repo: &str) -> RepoSpec {
    RepoSpec {
        org: org.to_string(),
        repo: repo.to_string(),
    }
}

/// A `Config` whose "remote" is the local `<root>/origin` directory, so the
/// transport clones `<root>/origin/<org>/<repo>` with no network.
fn fixture_config(root: &Path, org: &str, repo: &str) -> Config {
    Config {
        op: Op::Init(spec(org, repo)),
        default_branch: Some("main".to_string()),
        assume_yes: false,
        clonepath: root.join("work"),
        remote: root.join("origin").to_string_lossy().into_owned(),
        mirrorpath: None,
        verbose: false,
        dry_run: false,
        ssh_key: None,
    }
}

#[test]
fn test_init_produces_fresh_bare_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    let config = fixture_config(root, "org", "repo");
    let worktree = init(&config, &spec("org", "repo")).unwrap();

    let container = root.join("work").join("org").join("repo");
    assert!(container.join(".bare").is_dir(), ".bare dir should exist");
    assert_eq!(
        fs::read_to_string(container.join(".git")).unwrap(),
        "gitdir: ./.bare\n",
        ".git must be a relative pointer file"
    );
    assert_eq!(worktree, container.join("main"));
    assert!(
        worktree.join("README.md").is_file(),
        "default worktree should be checked out"
    );

    // origin/* populated by the refspec fix + fetch.
    let branches = git::output(&["branch", "-r"], Some(&container), None).unwrap();
    assert!(
        branches.stdout.contains("origin/main"),
        "origin/main remote-tracking branch must be populated; got: {:?}",
        branches.stdout
    );
}

#[test]
fn test_init_reconciles_existing_bare_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    let config = fixture_config(root, "org", "repo");
    let first = init(&config, &spec("org", "repo")).unwrap();

    // A second init on the now-existing bare container reconciles in place and
    // returns the same default worktree.
    let second = init(&config, &spec("org", "repo")).unwrap();
    assert_eq!(first, second, "re-init must be idempotent");
    assert!(second.join("README.md").is_file());
}

#[test]
fn test_init_updates_existing_flat_clone_in_place() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    // A pre-existing FLAT checkout at the target path (as if `clone` made it).
    let target = root.join("work").join("org").join("repo");
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let src = root.join("origin").join("org").join("repo");
    git_run(root, &["clone", src.to_str().unwrap(), target.to_str().unwrap()]);
    assert!(target.join(".git").is_dir(), "precondition: a flat checkout");

    let config = fixture_config(root, "org", "repo");
    let dest = init(&config, &spec("org", "repo")).unwrap();

    // init must NOT clobber a flat checkout into a bare container.
    assert_eq!(dest, target, "init returns the existing flat checkout path");
    assert!(target.join(".git").is_dir(), "still a flat checkout after init");
    assert!(
        !target.join(".bare").exists(),
        "flat checkout must not be converted to bare"
    );
}

#[test]
fn test_is_flat_clone_distinguishes_layouts() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    // A flat checkout is a flat clone.
    let flat = root.join("flat");
    let src = root.join("origin").join("org").join("repo");
    git_run(root, &["clone", src.to_str().unwrap(), flat.to_str().unwrap()]);
    assert!(is_flat_clone(&flat), "a real checkout is a flat clone");

    // A bare container is NOT a flat clone.
    let config = fixture_config(root, "org", "repo");
    init(&config, &spec("org", "repo")).unwrap();
    let container = root.join("work").join("org").join("repo");
    assert!(!is_flat_clone(&container), "a bare container is not a flat clone");

    // A plain empty dir is not a flat clone.
    let empty = root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    assert!(!is_flat_clone(&empty), "an empty dir is not a flat clone");
}
