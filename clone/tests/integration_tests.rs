use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn get_clone_binary() -> PathBuf {
    let mut path = env::current_exe().unwrap();
    path.pop(); // Remove test executable name
    path.pop(); // Remove 'deps' directory
    path.push("clone");
    path
}

fn create_temp_dir(test_name: &str) -> PathBuf {
    let temp_dir = env::temp_dir().join(format!("clone_test_{}", test_name));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();
    temp_dir
}

#[test]
fn test_clone_nonexistent_repo_fails_with_clear_error() {
    let temp_dir = create_temp_dir("nonexistent");
    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("nonexistent-org-12345/nonexistent-repo-67890")
        .output()
        .expect("Failed to execute clone command");

    assert!(!output.status.success(), "Command should fail for nonexistent repo");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to clone repository") || stderr.contains("Error:"),
        "Error message should mention clone failure. Got: {}",
        stderr
    );

    // The directory should not exist since clone failed
    assert!(
        !temp_dir.join("nonexistent-org-12345/nonexistent-repo-67890").exists(),
        "Directory should not exist after failed clone"
    );

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_clone_with_verbose_shows_detailed_errors() {
    let temp_dir = create_temp_dir("verbose");
    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("--verbose")
        .arg("nonexistent-org-99999/nonexistent-repo-88888")
        .output()
        .expect("Failed to execute clone command");

    assert!(!output.status.success(), "Command should fail for nonexistent repo");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to clone from"),
        "Verbose error should show 'Failed to clone from'. Got: {}",
        stderr
    );

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_clone_existing_public_repo_succeeds() {
    let temp_dir = create_temp_dir("public");
    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("rust-lang/libc")
        .output()
        .expect("Failed to execute clone command");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "Command should succeed for public repo. Stderr: {}, Stdout: {}",
        stderr, stdout
    );

    // Check that the directory was created and has a .git directory
    let cloned_dir = temp_dir.join("rust-lang/libc");
    assert!(
        cloned_dir.exists(),
        "Cloned directory should exist at {:?}",
        cloned_dir
    );
    assert!(
        cloned_dir.join(".git").exists(),
        ".git directory should exist in cloned repo"
    );

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_clone_already_exists_updates_repo() {
    let temp_dir = create_temp_dir("update");
    let binary = get_clone_binary();

    // First clone
    let output1 = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("rust-lang/libc")
        .output()
        .expect("Failed to execute first clone command");

    assert!(output1.status.success(), "First clone should succeed");

    // Second clone (should update instead of error)
    let output2 = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("rust-lang/libc")
        .output()
        .expect("Failed to execute second clone command");

    assert!(
        output2.status.success(),
        "Second clone should succeed (update). Stderr: {}",
        String::from_utf8_lossy(&output2.stderr)
    );

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_clone_with_custom_clonepath() {
    let temp_dir = create_temp_dir("custom_path");
    let custom_clone_path = temp_dir.join("custom");
    fs::create_dir_all(&custom_clone_path).unwrap();

    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .current_dir(&temp_dir)
        .arg("--clonepath")
        .arg(&custom_clone_path)
        .arg("rust-lang/libc")
        .output()
        .expect("Failed to execute clone command");

    assert!(
        output.status.success(),
        "Clone with custom path should succeed. Stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let cloned_dir = custom_clone_path.join("rust-lang/libc");
    assert!(
        cloned_dir.exists(),
        "Repo should be cloned to custom path at {:?}",
        cloned_dir
    );

    fs::remove_dir_all(&temp_dir).ok();
}

#[test]
fn test_help_command_works() {
    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .arg("--help")
        .output()
        .expect("Failed to execute --help command");

    assert!(output.status.success(), "--help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("repospec"), "Help should mention repospec");
    assert!(stdout.contains("revision"), "Help should mention revision");
}

#[test]
fn test_version_command_works() {
    let binary = get_clone_binary();

    let output = Command::new(&binary)
        .arg("--version")
        .output()
        .expect("Failed to execute --version command");

    assert!(output.status.success(), "--version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("clone") || !stdout.is_empty(), "Version should show something");
}

