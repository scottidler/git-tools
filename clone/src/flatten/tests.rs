use super::*;
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

fn commit(dir: &Path, msg: &str) {
    git_run(
        dir,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", msg],
    );
}

/// Whether `rkvr` is available; the copy-based transition removes via `rkvr rmrf`,
/// so full-collapse tests are gated on it (refuse/inspection tests are not).
fn rkvr_available() -> bool {
    std::process::Command::new("rkvr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a real bare container at `<root>/work/org/repo` with a `main`-branch
/// default worktree, cloned (no network) from a local bare "remote". Returns
/// `(container, remote)`. Mirrors what `clone --bare` produces: `.bare/` + `.git`
/// pointer + the non-bare fetch refspec + populated `origin/*` + a `main/` worktree.
fn make_container(root: &Path) -> (PathBuf, PathBuf) {
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

    let container = root.join("work").join("org").join("repo");
    fs::create_dir_all(container.parent().unwrap()).unwrap();
    let bare = container.join(".bare");
    git_run(
        root,
        &["clone", "--bare", remote.to_str().unwrap(), bare.to_str().unwrap()],
    );
    fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
    git_run(
        &container,
        &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"],
    );
    git_run(&container, &["fetch", "origin"]);
    git_run(&container, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git_run(&container, &["worktree", "add", "main", "main"]);
    (container, remote)
}

// ----------------------------------------------------------------------------
// Full collapse (gated on rkvr — the transition removes via `rkvr rmrf`)
// ----------------------------------------------------------------------------

#[test]
fn test_flatten_clean_single_main_container() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_clean_single_main_container: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, remote) = make_container(root);

    let flat = flatten(&container, Some("main")).unwrap();

    assert_eq!(flat, container.canonicalize().unwrap());
    assert!(!bare::is_bare_container(&container), "no more .bare - it is flat now");
    assert!(container.join(".git").is_dir(), "flat checkout has a real .git dir");
    assert!(container.join("README.md").is_file(), "tree materialized at the root");

    // Origin URL preserved.
    let origin = git::output(&["remote", "get-url", "origin"], Some(&container), None).unwrap();
    assert_eq!(origin.stdout.trim(), remote.to_str().unwrap());

    // Working tree functional and clean.
    let status = git::output(&["status", "--porcelain"], Some(&container), None).unwrap();
    assert!(status.status.success() && status.stdout.trim().is_empty());

    // No staging/backup dirs left behind.
    assert!(!root.join("work").join("org").join("repo.flattening").exists());
    assert!(!root.join("work").join("org").join("repo.flatten-backup").exists());
}

#[test]
fn test_flatten_merged_feature_worktree_preserves_all_refs() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_merged_feature_worktree_preserves_all_refs: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");

    // Feature branch at the current commit (ancestor of where main will move),
    // checked out in its own worktree; then advance main so feature is fully
    // merged (an ancestor of the default).
    git_run(&main_wt, &["branch", "feature"]);
    git_run(&container, &["worktree", "add", "feature", "feature"]);
    fs::write(main_wt.join("more.txt"), "more").unwrap();
    git_run(&main_wt, &["add", "."]);
    commit(&main_wt, "advance main");

    let before = refs_snapshot(&container).unwrap();
    assert!(before.contains_key("refs/heads/feature"));

    let flat = flatten(&container, Some("main")).unwrap();

    // Every ref under refs/* survives at an identical OID (the invariant).
    let after = refs_snapshot(&flat).unwrap();
    assert_eq!(after, before, "all refs/* OIDs must be identical after collapse");
    assert!(!bare::is_bare_container(&container));
    // The feature worktree dir is gone (archived) but its branch ref survives.
    assert!(
        !container.join("feature").exists(),
        "the feature worktree dir is archived"
    );
}

#[test]
fn test_flatten_staged_db_passes_fsck() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_staged_db_passes_fsck: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    // A successful flatten only commits after verify_flat runs fsck on the staged
    // DB; assert the resulting store is connectivity-clean directly too.
    let flat = flatten(&container, Some("main")).unwrap();
    let fsck = git::output(&["fsck", "--connectivity-only"], Some(&flat), None).unwrap();
    assert!(fsck.status.success(), "flattened store must pass fsck: {}", fsck.stderr);
}

#[test]
fn test_flatten_preserves_fetch_refspec() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_preserves_fetch_refspec: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    let flat = flatten(&container, Some("main")).unwrap();

    // The non-bare refspec must survive; we must NOT introduce
    // +refs/heads/*:refs/heads/* (it clobbers local branches on the next fetch).
    let refspec = git::output(&["config", "--get", "remote.origin.fetch"], Some(&flat), None).unwrap();
    assert_eq!(refspec.stdout.trim(), "+refs/heads/*:refs/remotes/origin/*");
    // And core.bare is off now.
    let bare_cfg = git::output(&["config", "--get", "core.bare"], Some(&flat), None).unwrap();
    assert_eq!(bare_cfg.stdout.trim(), "false");
}

#[test]
fn test_flatten_refuses_dirty_worktree_end_to_end() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_refuses_dirty_worktree_end_to_end: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    fs::write(container.join("main").join("README.md"), "dirty").unwrap();

    let err = flatten(&container, Some("main")).unwrap_err();
    assert!(
        format!("{err}").contains("uncommitted changes or untracked files"),
        "flatten must refuse a dirty worktree; got: {err}"
    );
    // Nothing changed - still a bare container.
    assert!(
        bare::is_bare_container(&container),
        "a refused flatten must not mutate the container"
    );
    assert!(container.join(".bare").is_dir());
}

#[test]
fn test_flatten_reports_ignored_files_and_still_collapses() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_reports_ignored_files_and_still_collapses: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");
    fs::write(main_wt.join(".gitignore"), ".env\n").unwrap();
    git_run(&main_wt, &["add", ".gitignore"]);
    commit(&main_wt, "add gitignore");
    fs::write(main_wt.join(".env"), "SECRET=1").unwrap();

    // Ignored file is detected (for the recoverable-archival report)...
    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.ignored
            .iter()
            .any(|(_, files)| files.iter().any(|f| f.contains(".env"))),
        "ignored .env must be detected; got {:?}",
        insp.ignored
    );

    // ...and does NOT block the collapse.
    let flat = flatten(&container, Some("main")).unwrap();
    assert!(
        !bare::is_bare_container(&container),
        "ignored files must not block collapse"
    );
    // The ignored file was archived with the container (not carried into the flat
    // root); it is recoverable from the rkvr archive of the whole container.
    assert!(
        !flat.join(".env").exists(),
        "ignored file is archived, not silently carried"
    );
}

/// Mid-transition failure (here: a non-existent default so `reset --hard` fails
/// before the swap) must leave the LIVE `.bare` intact — the transition copies,
/// never moves.
#[test]
fn test_flatten_transition_failure_leaves_live_bare_intact() {
    if !rkvr_available() {
        eprintln!("SKIP test_flatten_transition_failure_leaves_live_bare_intact: rkvr not available");
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let container = container.canonicalize().unwrap();
    let before = refs_snapshot(&container).unwrap();

    let err = perform_transition(&container, "no-such-default-branch", &before).unwrap_err();
    assert!(
        format!("{err:#}").contains("materializing the flat working tree"),
        "the transition must fail while materializing (before any swap); got: {err:#}"
    );

    // The live container is untouched: still bare, .bare present, refs intact.
    assert!(
        bare::is_bare_container(&container),
        "live container must survive a failed transition"
    );
    assert!(
        container.join(".bare").is_dir(),
        "live .bare must be intact (copy, not move)"
    );
    assert_eq!(refs_snapshot(&container).unwrap(), before, "live refs untouched");
}

// ----------------------------------------------------------------------------
// Refuse-first checks (via inspect — no rkvr needed; refusals collected)
// ----------------------------------------------------------------------------

#[test]
fn test_inspect_refuses_dirty_worktree() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    fs::write(container.join("main").join("README.md"), "dirty").unwrap();

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals
            .iter()
            .any(|r| r.contains("uncommitted changes or untracked files")),
        "must refuse a dirty tree; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_untracked_file() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    fs::write(container.join("main").join("new-untracked.txt"), "x").unwrap();

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals
            .iter()
            .any(|r| r.contains("uncommitted changes or untracked files")),
        "must refuse an untracked file; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_unmerged_but_clean_branch() {
    // The case the old `has_unmerged` (diff-filter=U) missed: a CLEAN worktree on
    // a branch with commits not in the default is refused by ancestry.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");

    git_run(&main_wt, &["branch", "feature"]);
    git_run(&container, &["worktree", "add", "feature", "feature"]);
    let feat_wt = container.join("feature");
    fs::write(feat_wt.join("unpushed.txt"), "local work").unwrap();
    git_run(&feat_wt, &["add", "."]);
    commit(&feat_wt, "unpushed feature commit");

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("not merged into the default")),
        "must refuse a clean-but-unmerged branch; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_detached_head_unreachable() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    // A detached worktree carrying a commit no ref points at.
    git_run(&container, &["worktree", "add", "--detach", "det", "main"]);
    let det = container.join("det");
    fs::write(det.join("unique.txt"), "detached work").unwrap();
    git_run(&det, &["add", "."]);
    commit(&det, "detached unique commit");

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("detached HEAD")),
        "must refuse an unreachable detached HEAD; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_existing_stash() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");

    // A stash leaves the tree clean but creates refs/stash.
    fs::write(main_wt.join("README.md"), "wip change").unwrap();
    git_run(&main_wt, &["stash", "push", "-m", "wip"]);

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("refs/stash")),
        "must refuse an existing stash; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_active_bisect() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    git_run(&container.join("main"), &["bisect", "start"]);

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("in-progress bisect")),
        "must refuse an active bisect; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_clean_container_has_no_refusals() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.is_empty(),
        "a clean single-main container must be safe; got {:?}",
        insp.refusals
    );
    assert_eq!(insp.default, "main");
    assert!(insp.refs_before.contains_key("refs/heads/main"));
}

// ----------------------------------------------------------------------------
// Per-worktree in-progress-operation detection (unit — no git gymnastics)
// ----------------------------------------------------------------------------

#[test]
fn test_in_progress_operation_detects_each_state() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let none = root.join("clean");
    fs::create_dir_all(&none).unwrap();
    assert_eq!(in_progress_operation(&none), None);

    for (name, is_dir, expected) in [
        ("MERGE_HEAD", false, "merge"),
        ("rebase-merge", true, "rebase"),
        ("rebase-apply", true, "rebase"),
        ("CHERRY_PICK_HEAD", false, "cherry-pick"),
        ("REVERT_HEAD", false, "revert"),
        ("BISECT_START", false, "bisect"),
        ("BISECT_LOG", false, "bisect"),
    ] {
        let g = root.join(format!("g-{name}"));
        fs::create_dir_all(&g).unwrap();
        let p = g.join(name);
        if is_dir {
            fs::create_dir_all(&p).unwrap();
        } else {
            fs::write(&p, "x").unwrap();
        }
        assert_eq!(
            in_progress_operation(&g),
            Some(expected),
            "state file {name} must be detected as {expected}"
        );
    }
}

#[test]
fn test_per_worktree_state_detects_config_and_sparse() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let clean = root.join("clean");
    fs::create_dir_all(&clean).unwrap();
    assert_eq!(per_worktree_state(&clean), None);

    let cfg = root.join("cfg");
    fs::create_dir_all(&cfg).unwrap();
    fs::write(cfg.join("config.worktree"), "[core]\n").unwrap();
    assert!(
        per_worktree_state(&cfg).is_some(),
        "per-worktree config must be detected"
    );

    let sparse = root.join("sparse");
    fs::create_dir_all(sparse.join("info")).unwrap();
    fs::write(sparse.join("info").join("sparse-checkout"), "/*\n").unwrap();
    assert!(
        per_worktree_state(&sparse).is_some(),
        "sparse-checkout must be detected"
    );
}

// ----------------------------------------------------------------------------
// copy_dir_all: copy, not move (live source stays intact)
// ----------------------------------------------------------------------------

#[test]
fn test_copy_dir_all_leaves_source_intact() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let src = root.join("src");
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("a.txt"), "alpha").unwrap();
    fs::write(src.join("nested").join("b.txt"), "beta").unwrap();

    let dst = root.join("dst");
    copy_dir_all(&src, &dst).unwrap();

    // Destination is a faithful copy.
    assert_eq!(fs::read_to_string(dst.join("a.txt")).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(dst.join("nested").join("b.txt")).unwrap(), "beta");
    // Source is untouched (copy, not move).
    assert!(src.join("a.txt").is_file(), "source must remain after a copy");
    assert!(src.join("nested").join("b.txt").is_file());
}

// ----------------------------------------------------------------------------
// container resolution + dry-run
// ----------------------------------------------------------------------------

#[test]
fn test_container_from_dir_resolves_from_worktree_and_subdir() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let canonical = container.canonicalize().unwrap();
    let main_wt = container.join("main");

    assert_eq!(
        container_from_dir(&main_wt).unwrap().canonicalize().unwrap(),
        canonical,
        "resolves the container from its default worktree"
    );

    let sub = main_wt.join("a").join("b");
    fs::create_dir_all(&sub).unwrap();
    assert_eq!(
        container_from_dir(&sub).unwrap().canonicalize().unwrap(),
        canonical,
        "resolves the container from a subdirectory"
    );
}

#[test]
fn test_container_from_dir_rejects_flat_checkout() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (_, remote) = make_container(root);

    let flat = root.join("flat");
    git_run(root, &["clone", remote.to_str().unwrap(), flat.to_str().unwrap()]);

    let err = container_from_dir(&flat).unwrap_err();
    assert!(
        format!("{err}").contains("not a bare container"),
        "must reject a flat checkout; got: {err}"
    );
}

#[test]
fn test_dry_run_makes_no_changes() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    // A refuse condition, to exercise the refusal-reporting path.
    fs::write(container.join("main").join("README.md"), "dirty").unwrap();

    let result = dry_run(&container, Some("main")).unwrap();

    assert_eq!(result, container.canonicalize().unwrap());
    assert!(bare::is_bare_container(&container), "dry-run must not collapse");
    assert!(container.join(".bare").is_dir(), "still a bare container");
    assert!(!root.join("work").join("org").join("repo.flattening").exists());
    assert!(!root.join("work").join("org").join("repo.flatten-backup").exists());
}
