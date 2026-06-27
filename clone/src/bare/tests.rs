use super::*;
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

/// Build a local source repo with one commit on `main` at `<root>/origin/<org>/<repo>`.
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

/// A `Config` whose "remote" is the local `<root>/origin` directory, so
/// `transport::clone_with_fallback` clones `<root>/origin/<org>/<repo>`.
fn fixture_config(root: &Path, org: &str, repo: &str) -> Config {
    Config {
        spec: Some(spec(org, repo)),
        op: Op::Clone,
        layout: Layout::Bare,
        revision: "HEAD".to_string(),
        remote: root.join("origin").to_string_lossy().into_owned(),
        clonepath: root.join("work"),
        mirrorpath: None,
        versioning: false,
        verbose: false,
        dry_run: false,
        ssh_key: None,
        default_branch: Some("main".to_string()),
    }
}

#[test]
fn test_setup_bare_container_layout() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "myorg", "myrepo");

    let config = fixture_config(root, "myorg", "myrepo");
    let worktree = setup_bare_container(&config, &spec("myorg", "myrepo")).unwrap();

    let container = root.join("work").join("myorg").join("myrepo");
    assert!(container.join(".bare").is_dir(), ".bare dir should exist");

    let pointer = fs::read_to_string(container.join(".git")).unwrap();
    assert_eq!(pointer, "gitdir: ./.bare\n");

    assert_eq!(worktree, container.join("main"));
    assert!(worktree.join("README.md").is_file(), "worktree should be checked out");
    assert!(is_bare_container(&container));
}

#[test]
fn test_setup_populates_remote_tracking() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    let config = fixture_config(root, "org", "repo");
    setup_bare_container(&config, &spec("org", "repo")).unwrap();

    let container = root.join("work").join("org").join("repo");
    // The refspec fix + fetch must populate origin/*.
    let out = git::output(&["branch", "-r"], Some(&container), None).unwrap();
    assert!(
        out.stdout.contains("origin/main"),
        "remote-tracking branch missing; got: {:?}",
        out.stdout
    );
}

#[test]
fn test_default_branch_detected() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    let config = fixture_config(root, "org", "repo");
    setup_bare_container(&config, &spec("org", "repo")).unwrap();

    let container = root.join("work").join("org").join("repo");
    assert_eq!(default_branch(&container, None).unwrap(), "main");
}

#[test]
fn test_reconcile_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    make_source(root, "org", "repo");

    let config = fixture_config(root, "org", "repo");
    let first = setup_bare_container(&config, &spec("org", "repo")).unwrap();

    // Re-running reconcile must not error and must return the same worktree.
    let second = reconcile_container(&config, &root.join("work").join("org").join("repo")).unwrap();
    assert_eq!(first, second);
    assert!(second.join("README.md").is_file());
}

/// Security-relevant invariant: a worktree placed under `~/repos/tatari-tv/`
/// resolves to the work identity via gitconfig `includeIf "gitdir:"`. Locks the
/// persona property so a refactor can't silently break it.
#[test]
fn test_persona_invariant_under_org_prefix() {
    let tmp = TempDir::new().unwrap();
    // Canonicalize so the gitdir pattern matches the real (symlink-free) path.
    let root = tmp.path().canonicalize().unwrap();

    // Mimic ~/repos/tatari-tv/<repo>.
    let repos = root.join("repos");
    make_source(&repos, "tatari-tv", "svc");

    let mut config = fixture_config(&repos, "tatari-tv", "svc");
    // Clone the container directly under repos/tatari-tv/ (the org prefix).
    config.clonepath = repos.clone();
    let worktree = setup_bare_container(&config, &spec("tatari-tv", "svc")).unwrap();

    // A global gitconfig that swaps in the work identity for anything under
    // repos/tatari-tv/.
    let work_cfg = root.join("gitconfig-work");
    fs::write(&work_cfg, "[user]\n\temail = escote@tatari.tv\n").unwrap();
    let global_cfg = root.join("gitconfig");
    fs::write(
        &global_cfg,
        format!(
            "[user]\n\temail = scott@home.com\n[includeIf \"gitdir:{}/repos/tatari-tv/\"]\n\tpath = {}\n",
            root.display(),
            work_cfg.display()
        ),
    )
    .unwrap();

    let global = global_cfg.to_string_lossy();
    let out = git::output(
        &["config", "user.email"],
        Some(&worktree),
        Some(&[("GIT_CONFIG_GLOBAL", &global), ("GIT_CONFIG_SYSTEM", "/dev/null")]),
    )
    .unwrap();

    assert_eq!(
        out.stdout.trim(),
        "escote@tatari.tv",
        "worktree under the org prefix must resolve to the work identity"
    );
}
