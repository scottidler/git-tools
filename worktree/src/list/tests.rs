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

#[test]
fn test_parse_skips_bare_and_reads_branches() {
    // A representative porcelain stream: the bare entry plus two worktrees,
    // one of which is detached.
    let porcelain = "\
worktree /repos/org/repo/.bare
bare

worktree /repos/org/repo/main
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /repos/org/repo/feature-auth
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature/auth

worktree /repos/org/repo/detached
HEAD 3333333333333333333333333333333333333333
detached

worktree /repos/org/repo/pinned
HEAD 4444444444444444444444444444444444444444
branch refs/heads/pinned
locked on purpose
";
    let entries = parse(porcelain);
    assert_eq!(entries.len(), 5);

    assert!(entries[0].bare, "first entry is the bare repo");
    assert_eq!(entries[1].branch.as_deref(), Some("main"));
    assert_eq!(
        entries[2].branch.as_deref(),
        Some("feature/auth"),
        "refs/heads/ stripped, slash kept"
    );
    assert_eq!(entries[3].branch, None, "detached HEAD has no branch");
    assert_eq!(entries[3].path, PathBuf::from("/repos/org/repo/detached"));
    assert!(!entries[3].locked, "detached entry is not locked");
    assert!(entries[4].locked, "the `locked <reason>` line sets locked");
    assert_eq!(entries[4].branch.as_deref(), Some("pinned"));
}

#[test]
fn test_parse_empty_is_empty() {
    assert!(parse("").is_empty());
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
