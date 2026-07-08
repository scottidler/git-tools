// worktree — e2e lifecycle tests: drive the BUILT BINARY through the bare
// lifecycle (`init` for fresh acquisition, `migrate` flat->bare, `flatten`
// bare->flat) against a local bare "remote" fixture. `git ls-remote`/`fetch`
// against a local filesystem path work offline, so these are hermetic (no
// network) even though `migrate` probes connectivity and `migrate`/`flatten`
// require `rkvr` in preflight.
//
// Ported from clone's `tests/lifecycle_tests.rs` (which drove `clone --bare/
// --migrate/--flatten`) as part of the clone/worktree split: the acquisition +
// layout surface now lives on `worktree`, so its e2e round trip lives here.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Content of the seed repo's tracked file, asserted byte-for-byte after every
/// transition - proves the round trip carries content, not just structure.
const MARKER_CONTENT: &str = "e2e-lifecycle-marker\n";

fn get_worktree_binary() -> PathBuf {
    let mut path = env::current_exe().unwrap();
    path.pop(); // Remove test executable name
    path.pop(); // Remove 'deps' directory
    path.push("worktree");
    path
}

fn create_temp_dir(test_name: &str) -> PathBuf {
    let temp_dir = env::temp_dir().join(format!("worktree_lifecycle_test_{}", test_name));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();
    temp_dir
}

fn git_run(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {:?} in {}: {}", args, dir.display(), e));
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("failed to run git {:?} in {}: {}", args, dir.display(), e));
    assert!(
        output.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(dir: &Path, msg: &str) {
    git_run(
        dir,
        &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", msg],
    );
}

/// Sorted `refname objectname` lines under `refs/*`, for a before/after OID-set
/// equality assertion across the migrate -> flatten round trip.
fn for_each_ref(dir: &Path) -> Vec<String> {
    let mut refs: Vec<String> = git_stdout(dir, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .map(str::to_string)
        .collect();
    refs.sort();
    refs
}

/// Whether `rkvr` is available. `migrate`/`flatten` require it in preflight
/// (`common::rkvr::require`), so those tests are gated on it. `init` does not.
fn rkvr_available() -> bool {
    Command::new("rkvr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a local bare "remote" at `<root>/remotes/<org>/<repo>`, seeded with one
/// commit on `main` carrying `MARKER.txt`. Returns the `--remote` root: the
/// parent directory the tool joins `<org>/<repo>` onto to form the clone URL.
fn make_remote(root: &Path, org: &str, repo: &str) -> PathBuf {
    let seed = root.join("seed").join(repo);
    fs::create_dir_all(&seed).unwrap();
    git_run(&seed, &["init", "-b", "main"]);
    fs::write(seed.join("MARKER.txt"), MARKER_CONTENT).unwrap();
    git_run(&seed, &["add", "."]);
    commit(&seed, "init");

    let remote_root = root.join("remotes");
    let remote = remote_root.join(org).join(repo);
    fs::create_dir_all(remote.parent().unwrap()).unwrap();
    git_run(
        root,
        &["clone", "--bare", seed.to_str().unwrap(), remote.to_str().unwrap()],
    );
    remote_root
}

/// A flat checkout of `<remote_root>/<org>/<repo>` at `<work>/<org>/<repo>`,
/// mirroring the layout `worktree migrate --clonepath <work> <org>/<repo>` targets.
fn make_flat_checkout(root: &Path, remote_root: &Path, work: &Path, org: &str, repo: &str) -> PathBuf {
    let target = work.join(org).join(repo);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    let src = remote_root.join(org).join(repo);
    git_run(root, &["clone", src.to_str().unwrap(), target.to_str().unwrap()]);
    target
}

/// Run the built `worktree` binary with `args` from `cwd`.
fn run_worktree(cwd: &Path, args: &[&str]) -> Output {
    Command::new(get_worktree_binary())
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("failed to execute worktree binary")
}

#[test]
fn test_e2e_init_produces_bare_container_from_outside_a_repo() {
    // `worktree init` from a temp dir OUTSIDE any repo must produce `.bare/` + a
    // relative `.git` pointer + populated `origin/*` + a default-branch worktree.
    let tmp = create_temp_dir("init");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    let out = run_worktree(
        &tmp,
        &[
            "init",
            "--remote",
            remote_root.to_str().unwrap(),
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        out.status.success(),
        "worktree init should succeed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let container = work.join(org).join(repo);
    assert!(container.join(".bare").is_dir(), "init should create a .bare dir");
    assert_eq!(
        fs::read_to_string(container.join(".git")).unwrap(),
        "gitdir: ./.bare\n",
        ".git must be a relative pointer file"
    );
    let default_wt = container.join("main");
    assert!(
        default_wt.is_dir(),
        "default-branch worktree should exist at {:?}",
        default_wt
    );
    assert_eq!(
        fs::read_to_string(default_wt.join("MARKER.txt")).unwrap(),
        MARKER_CONTENT,
        "content should be present in the default worktree"
    );

    // origin/* populated.
    let branches = git_stdout(&container, &["branch", "-r"]);
    assert!(
        branches.contains("origin/main"),
        "origin/main must be populated; got: {}",
        branches
    );

    // stdout is the destination path the wrapper cd's into.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        PathBuf::from(stdout.trim()),
        default_wt,
        "init must print the default worktree path to stdout"
    );

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_e2e_init_help_prints_usage() {
    // `worktree init --help` must print usage (clap), not a git-clone error.
    let tmp = create_temp_dir("init_help");
    let out = run_worktree(&tmp, &["init", "--help"]);
    assert!(out.status.success(), "init --help should exit 0");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Usage") && stdout.contains("--clonepath"),
        "init --help must print usage listing --clonepath; got:\n{}",
        stdout
    );
    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_e2e_migrate_flat_to_bare_by_explicit_spec() {
    // `worktree migrate <spec>` from outside any repo (explicit spec, no
    // cwd/main-worktree resolution) converts the flat checkout to a bare container.
    if !rkvr_available() {
        eprintln!("SKIP test_e2e_migrate_flat_to_bare_by_explicit_spec: rkvr not available");
        return;
    }
    let tmp = create_temp_dir("migrate");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    let checkout = make_flat_checkout(&tmp, &remote_root, &work, org, repo);
    assert!(
        checkout.join(".git").is_dir(),
        "flat checkout should have a real .git dir"
    );

    let migrate_out = run_worktree(&tmp, &["migrate", "--clonepath", work.to_str().unwrap(), &repospec]);
    assert!(
        migrate_out.status.success(),
        "migrate should succeed: {}",
        String::from_utf8_lossy(&migrate_out.stderr)
    );

    assert!(checkout.join(".bare").is_dir(), "migrated repo should have a .bare dir");
    assert!(
        checkout.join(".git").is_file(),
        "migrated repo should have a .git pointer file"
    );
    let default_wt = checkout.join("main");
    assert!(
        default_wt.is_dir(),
        "default-branch worktree should exist at {:?}",
        default_wt
    );
    assert_eq!(
        fs::read_to_string(default_wt.join("MARKER.txt")).unwrap(),
        MARKER_CONTENT,
        "content should survive the flat -> bare migration"
    );

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_e2e_migrate_then_flatten_round_trip_oid_equality() {
    // The success criterion: `worktree migrate` then `worktree flatten` yields an
    // identical `git for-each-ref` OID set before and after the collapse. A
    // local-only branch (never pushed, no worktree) must survive purely by the
    // refs/* retention proof.
    if !rkvr_available() {
        eprintln!("SKIP test_e2e_migrate_then_flatten_round_trip_oid_equality: rkvr not available");
        return;
    }
    let tmp = create_temp_dir("roundtrip");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    let checkout = make_flat_checkout(&tmp, &remote_root, &work, org, repo);

    // A local-only branch with a unique commit, then back to main (clean tree).
    git_run(&checkout, &["checkout", "-b", "local-only"]);
    fs::write(checkout.join("local.txt"), "local-only work\n").unwrap();
    git_run(&checkout, &["add", "."]);
    commit(&checkout, "local-only commit");
    let local_oid = git_stdout(&checkout, &["rev-parse", "refs/heads/local-only"]);
    git_run(&checkout, &["checkout", "main"]);

    // Forward: flat -> bare.
    let migrate_out = run_worktree(&tmp, &["migrate", "--clonepath", work.to_str().unwrap(), &repospec]);
    assert!(
        migrate_out.status.success(),
        "migrate should succeed: {}",
        String::from_utf8_lossy(&migrate_out.stderr)
    );
    assert!(
        checkout.join(".bare").is_dir(),
        "migrated repo should be a bare container"
    );
    assert_eq!(
        git_stdout(&checkout, &["rev-parse", "refs/heads/local-only"]),
        local_oid,
        "local-only branch must survive the forward migrate"
    );

    // Snapshot the container's refs immediately before the collapse.
    let refs_before = for_each_ref(&checkout);

    // Reverse: bare -> flat.
    let flatten_out = run_worktree(&tmp, &["flatten", "--clonepath", work.to_str().unwrap(), &repospec]);
    assert!(
        flatten_out.status.success(),
        "flatten should succeed: {}",
        String::from_utf8_lossy(&flatten_out.stderr)
    );
    assert!(
        checkout.join(".git").is_dir(),
        "flattened repo should have a real .git dir"
    );
    assert!(
        !checkout.join(".bare").exists(),
        "flattened repo should have no .bare dir"
    );

    // The OID set must be identical before and after the flatten (no ref lost).
    let refs_after = for_each_ref(&checkout);
    assert_eq!(
        refs_after, refs_before,
        "for-each-ref OID set must be identical across the flatten"
    );
    assert_eq!(
        git_stdout(&checkout, &["rev-parse", "refs/heads/local-only"]),
        local_oid,
        "local-only branch must survive the full migrate -> flatten round trip"
    );
    assert_eq!(
        fs::read_to_string(checkout.join("MARKER.txt")).unwrap(),
        MARKER_CONTENT,
        "content must survive the full flat -> bare -> flat round trip"
    );

    fs::remove_dir_all(&tmp).ok();
}

#[test]
fn test_e2e_flatten_dry_run_makes_no_changes() {
    if !rkvr_available() {
        eprintln!("SKIP test_e2e_flatten_dry_run_makes_no_changes: rkvr not available");
        return;
    }
    let tmp = create_temp_dir("flatten_dry_run");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    // Produce a bare container via init.
    let init_out = run_worktree(
        &tmp,
        &[
            "init",
            "--remote",
            remote_root.to_str().unwrap(),
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        init_out.status.success(),
        "init should succeed: {}",
        String::from_utf8_lossy(&init_out.stderr)
    );
    let container = work.join(org).join(repo);
    assert!(container.join(".bare").is_dir());

    let dry = run_worktree(
        &tmp,
        &["flatten", "--dry-run", "--clonepath", work.to_str().unwrap(), &repospec],
    );
    assert!(
        dry.status.success(),
        "flatten --dry-run should succeed: {}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(
        dry.stdout.is_empty(),
        "flatten --dry-run must leave stdout empty (so the wrapper never cd's); got: {:?}",
        String::from_utf8_lossy(&dry.stdout)
    );
    assert!(
        String::from_utf8_lossy(&dry.stderr).contains("DRY RUN"),
        "flatten --dry-run must print its preview to stderr"
    );

    // Nothing changed: still a bare container, no staging/backup leftovers.
    assert!(
        container.join(".bare").is_dir(),
        "dry-run must not collapse the container"
    );
    assert!(
        container.join(".git").is_file(),
        "dry-run must not touch the .git pointer"
    );
    assert!(
        container.join("main").is_dir(),
        "dry-run must not remove the default worktree"
    );
    assert!(!work.join(org).join(format!("{}.flattening", repo)).exists());
    assert!(!work.join(org).join(format!("{}.flatten-backup", repo)).exists());

    fs::remove_dir_all(&tmp).ok();
}
