use super::*;
use common::git;
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

#[test]
fn test_list_real_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Minimal real bare container with a `main` worktree.
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    git_run(&src, &["init", "-b", "main"]);
    fs::write(src.join("README.md"), "hi").unwrap();
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
    git_run(&container, &["worktree", "add", "main", "main"]);

    let entries = list(&container).unwrap();
    let branches: Vec<_> = entries
        .iter()
        .filter(|e| !e.bare)
        .filter_map(|e| e.branch.clone())
        .collect();
    assert!(
        branches.contains(&"main".to_string()),
        "main worktree should be listed; got {branches:?}"
    );
}
