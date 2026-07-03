// clone — e2e lifecycle tests: drive the BUILT BINARY through the layout
// lifecycle (flat -> bare via --migrate, bare -> flat via --flatten) against a
// local bare "remote" fixture. `git ls-remote`/`fetch` against a local
// filesystem path work offline, so these are hermetic (no network) even though
// `--migrate` probes connectivity and `--flatten` requires `rkvr` in preflight.
//
// This complements `integration_tests.rs` (flat-default / --bare / --flat) and
// the module-level unit tests in `src/migrate/tests.rs` / `src/flatten/tests.rs`
// (which exercise the library functions directly): this file is the missing
// binary-level round trip through the actual CLI.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Content of the seed repo's tracked file, asserted byte-for-byte after every
/// transition - the thing that proves the round trip carries content, not just
/// structure.
const MARKER_CONTENT: &str = "e2e-lifecycle-marker\n";

fn get_clone_binary() -> PathBuf {
    let mut path = env::current_exe().unwrap();
    path.pop(); // Remove test executable name
    path.pop(); // Remove 'deps' directory
    path.push("clone");
    path
}

fn create_temp_dir(test_name: &str) -> PathBuf {
    let temp_dir = env::temp_dir().join(format!("clone_lifecycle_test_{}", test_name));
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

/// Whether `rkvr` is available. Both `--migrate` and `--flatten` require it in
/// preflight (`common::rkvr::require`), so every test in this file is gated on
/// it, mirroring `src/flatten/tests.rs`'s `rkvr_available` pattern.
fn rkvr_available() -> bool {
    Command::new("rkvr")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a local bare "remote" at `<root>/remotes/<org>/<repo>`, seeded with one
/// commit on `main` carrying `MARKER.txt`. Returns the `--remote` root: the
/// parent directory `clone` joins `<org>/<repo>` onto to form the clone URL. A
/// local filesystem path works as a `git clone`/`ls-remote`/`fetch` source with
/// no network, which is what keeps every test below hermetic.
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

/// Run the built `clone` binary with `args` from `cwd`.
fn run_clone(cwd: &Path, args: &[&str]) -> Output {
    Command::new(get_clone_binary())
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("failed to execute clone binary")
}

#[test]
fn test_e2e_migrate_flat_to_bare_container() {
    if !rkvr_available() {
        eprintln!("SKIP test_e2e_migrate_flat_to_bare_container: rkvr not available");
        return;
    }
    let tmp = create_temp_dir("migrate");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    // Flat clone (the new default layout) from the local remote.
    let clone_out = run_clone(
        &tmp,
        &[
            "--remote",
            remote_root.to_str().unwrap(),
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        clone_out.status.success(),
        "flat clone should succeed: {}",
        String::from_utf8_lossy(&clone_out.stderr)
    );
    let checkout = work.join(org).join(repo);
    assert!(checkout.join(".git").is_dir(), "flat clone should have a real .git dir");
    assert!(!checkout.join(".bare").exists(), "flat clone should have no .bare dir");

    // Forward migration: flat -> bare, addressed by an explicit repospec (no
    // cwd/`main worktree` resolution involved).
    let migrate_out = run_clone(&tmp, &["--migrate", "--clonepath", work.to_str().unwrap(), &repospec]);
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
fn test_e2e_flatten_round_trip_preserves_content() {
    if !rkvr_available() {
        eprintln!("SKIP test_e2e_flatten_round_trip_preserves_content: rkvr not available");
        return;
    }
    let tmp = create_temp_dir("flatten_roundtrip");
    let (org, repo) = ("e2eorg", "e2erepo");
    let remote_root = make_remote(&tmp, org, repo);
    let work = tmp.join("work");
    fs::create_dir_all(&work).unwrap();
    let repospec = format!("{}/{}", org, repo);

    // Produce a bare container directly via the --bare opt-in.
    let bare_out = run_clone(
        &tmp,
        &[
            "--bare",
            "--remote",
            remote_root.to_str().unwrap(),
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        bare_out.status.success(),
        "bare clone should succeed: {}",
        String::from_utf8_lossy(&bare_out.stderr)
    );
    let container = work.join(org).join(repo);
    assert!(container.join(".bare").is_dir());
    assert!(container.join(".git").is_file());
    assert_eq!(
        fs::read_to_string(container.join("main").join("MARKER.txt")).unwrap(),
        MARKER_CONTENT,
        "sanity: the bare container's default worktree has the seeded file before collapsing"
    );

    // Reverse round trip: bare -> flat via --flatten.
    let flatten_out = run_clone(&tmp, &["--flatten", "--clonepath", work.to_str().unwrap(), &repospec]);
    assert!(
        flatten_out.status.success(),
        "flatten should succeed: {}",
        String::from_utf8_lossy(&flatten_out.stderr)
    );

    assert!(
        container.join(".git").is_dir(),
        "flattened repo should have a real .git dir"
    );
    assert!(
        !container.join(".bare").exists(),
        "flattened repo should have no .bare dir"
    );
    assert!(
        !container.join("main").exists(),
        "the default-branch worktree dir collapses into the root"
    );

    // Content survives the full flat -> bare -> flat cycle, materialized at the
    // checkout root (not nested under a worktree dir).
    assert_eq!(
        fs::read_to_string(container.join("MARKER.txt")).unwrap(),
        MARKER_CONTENT,
        "content must survive the full flat -> bare -> flat round trip"
    );

    // The working tree is functional and clean.
    let status = git_stdout(&container, &["status", "--porcelain"]);
    assert!(status.is_empty(), "flattened checkout should be clean; got: {}", status);

    // Origin URL preserved through the round trip.
    let origin = git_stdout(&container, &["remote", "get-url", "origin"]);
    let expected_remote = remote_root.join(org).join(repo);
    assert_eq!(
        PathBuf::from(&origin),
        expected_remote,
        "origin URL should be preserved through the round trip"
    );

    // No leftover staging/backup dirs from the transition.
    assert!(!work.join(org).join(format!("{}.flattening", repo)).exists());
    assert!(!work.join(org).join(format!("{}.flatten-backup", repo)).exists());

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

    let bare_out = run_clone(
        &tmp,
        &[
            "--bare",
            "--remote",
            remote_root.to_str().unwrap(),
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        bare_out.status.success(),
        "bare clone should succeed: {}",
        String::from_utf8_lossy(&bare_out.stderr)
    );
    let container = work.join(org).join(repo);
    assert!(container.join(".bare").is_dir());

    let dry_run_out = run_clone(
        &tmp,
        &[
            "--flatten",
            "--dry-run",
            "--clonepath",
            work.to_str().unwrap(),
            &repospec,
        ],
    );
    assert!(
        dry_run_out.status.success(),
        "--flatten --dry-run should succeed: {}",
        String::from_utf8_lossy(&dry_run_out.stderr)
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
