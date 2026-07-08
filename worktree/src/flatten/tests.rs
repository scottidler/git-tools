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

/// Test remover: never invokes `rkvr`. `require` always succeeds; `rmrf` removes
/// via plain `std::fs` (no-op on a missing path). Lets the full-collapse tests run
/// without the real `rkvr` binary and without touching its archive.
struct FsRemover;

impl common::rkvr::Remover for FsRemover {
    fn require(&self) -> eyre::Result<()> {
        Ok(())
    }
    fn rmrf(&self, path: &Path) -> eyre::Result<()> {
        match path.symlink_metadata() {
            Err(_) => Ok(()),
            Ok(meta) if meta.is_dir() => Ok(fs::remove_dir_all(path)?),
            Ok(_) => Ok(fs::remove_file(path)?),
        }
    }
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
// Full collapse (inject FsRemover — the transition's removals go through the
// injected Remover, so no `rkvr` binary is needed and its archive is untouched)
// ----------------------------------------------------------------------------

#[test]
fn test_flatten_clean_single_main_container() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, remote) = make_container(root);

    let flat = flatten(&container, Some("main"), &FsRemover).unwrap();

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

    let flat = flatten(&container, Some("main"), &FsRemover).unwrap();

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
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    // A successful flatten only commits after verify_flat runs fsck on the staged
    // DB; assert the resulting store is connectivity-clean directly too.
    let flat = flatten(&container, Some("main"), &FsRemover).unwrap();
    let fsck = git::output(&["fsck", "--connectivity-only"], Some(&flat), None).unwrap();
    assert!(fsck.status.success(), "flattened store must pass fsck: {}", fsck.stderr);
}

#[test]
fn test_flatten_preserves_fetch_refspec() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    let flat = flatten(&container, Some("main"), &FsRemover).unwrap();

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
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    fs::write(container.join("main").join("README.md"), "dirty").unwrap();

    let err = flatten(&container, Some("main"), &FsRemover).unwrap_err();
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
    let flat = flatten(&container, Some("main"), &FsRemover).unwrap();
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
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let container = container.canonicalize().unwrap();
    let before = refs_snapshot(&container).unwrap();

    let err = perform_transition(&container, "no-such-default-branch", &before, &FsRemover).unwrap_err();
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
fn test_inspect_refuses_in_progress_cherry_pick() {
    // A REAL interrupted cherry-pick (not a synthetic CHERRY_PICK_HEAD file):
    // conflicting commits leave the main worktree mid-cherry-pick, which
    // in_progress_operation must detect through inspect.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");

    // base commit carrying c.txt
    fs::write(main_wt.join("c.txt"), "A\n").unwrap();
    git_run(&main_wt, &["add", "."]);
    commit(&main_wt, "base");
    // divergent branch 'other' changes c.txt
    git_run(&main_wt, &["checkout", "-b", "other"]);
    fs::write(main_wt.join("c.txt"), "B\n").unwrap();
    git_run(&main_wt, &["add", "."]);
    commit(&main_wt, "other change");
    // back on main, a conflicting change to the same line
    git_run(&main_wt, &["checkout", "main"]);
    fs::write(main_wt.join("c.txt"), "C\n").unwrap();
    git_run(&main_wt, &["add", "."]);
    commit(&main_wt, "main change");
    // cherry-pick 'other' onto main -> conflict, stops mid-op (CHERRY_PICK_HEAD).
    let cp = git::output(
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "cherry-pick", "other"],
        Some(&main_wt),
        None,
    )
    .unwrap();
    assert!(!cp.status.success(), "cherry-pick should conflict and stop");

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("in-progress cherry-pick")),
        "must refuse a real in-progress cherry-pick; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_dirty_submodule() {
    // A submodule whose checked-out HEAD is ahead of the recorded gitlink reports
    // `+` from `git submodule status` and must block the collapse.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);
    let main_wt = container.join("main");

    // A standalone repo to embed as a submodule.
    let sub_src = root.join("sub-src");
    fs::create_dir_all(&sub_src).unwrap();
    git_run(&sub_src, &["init", "-b", "main"]);
    fs::write(sub_src.join("s.txt"), "one").unwrap();
    git_run(&sub_src, &["add", "."]);
    commit(&sub_src, "sub init");

    // Add it as a submodule of the main worktree (file:// transport must be
    // explicitly allowed on modern git), then commit the gitlink.
    git_run(
        &main_wt,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            sub_src.to_str().unwrap(),
            "sub",
        ],
    );
    commit(&main_wt, "add submodule");

    // Advance the submodule's checked-out HEAD past the recorded gitlink -> `+`.
    let sub_wt = main_wt.join("sub");
    fs::write(sub_wt.join("s.txt"), "two").unwrap();
    git_run(&sub_wt, &["add", "."]);
    commit(&sub_wt, "advance submodule");

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals.iter().any(|r| r.contains("submodule")),
        "must refuse a dirty submodule; got {:?}",
        insp.refusals
    );
}

#[test]
fn test_inspect_refuses_when_safety_check_errors() {
    // Fail-closed: when a per-worktree safety check cannot even be evaluated (its
    // git invocation errors), the collapse must REFUSE, not proceed. A linked
    // worktree's directory is removed out from under its admin entry, so
    // is_dirty / worktree_gitdir / dirty_submodules all error on that worktree.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (container, _) = make_container(root);

    // A linked worktree at main's HEAD (an ancestor of the default, so ancestry
    // alone would not refuse it), then blow away its working directory.
    git_run(&container, &["worktree", "add", "gone", "-b", "gone"]);
    fs::remove_dir_all(container.join("gone")).unwrap();

    let insp = inspect(&container, Some("main")).unwrap();
    assert!(
        insp.refusals
            .iter()
            .any(|r| r.contains("could not be checked for uncommitted changes")),
        "is_dirty error must refuse (fail-closed); got {:?}",
        insp.refusals
    );
    assert!(
        insp.refusals
            .iter()
            .any(|r| r.contains("git dir could not be resolved")),
        "worktree_gitdir error must refuse (fail-closed); got {:?}",
        insp.refusals
    );
    assert!(
        insp.refusals
            .iter()
            .any(|r| r.contains("submodule state could not be determined")),
        "dirty_submodules error must refuse (fail-closed); got {:?}",
        insp.refusals
    );
    assert!(
        insp.refusals.iter().all(|r| r.contains("fail-closed")),
        "every refusal in this fixture is a fail-closed one; got {:?}",
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

    let result = dry_run(&container, Some("main"), &FsRemover).unwrap();

    assert_eq!(result, container.canonicalize().unwrap());
    assert!(bare::is_bare_container(&container), "dry-run must not collapse");
    assert!(container.join(".bare").is_dir(), "still a bare container");
    assert!(!root.join("work").join("org").join("repo.flattening").exists());
    assert!(!root.join("work").join("org").join("repo.flatten-backup").exists());
}
