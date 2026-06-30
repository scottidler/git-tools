use super::*;

// ---- init_script: zsh happy path ----------------------------------------

#[test]
fn zsh_script_defines_clone_function() {
    let script = init_script("zsh").expect("zsh should be supported");
    assert!(
        script.contains("clone()"),
        "emitted script should define the clone() function; got:\n{script}"
    );
}

#[test]
fn zsh_script_uses_command_clone() {
    let script = init_script("zsh").expect("zsh should be supported");
    assert!(
        script.contains("command clone"),
        "emitted script should use 'command clone', not a bare 'clone' or '$CLONE'; got:\n{script}"
    );
}

#[test]
fn zsh_script_does_not_use_dollar_clone_variable() {
    let script = init_script("zsh").expect("zsh should be supported");
    assert!(
        !script.contains("$CLONE"),
        "emitted script must not reference $CLONE; got:\n{script}"
    );
}

#[test]
fn zsh_script_carries_install_line() {
    let script = init_script("zsh").expect("zsh should be supported");
    // The install line must use `command clone` and a `hash` guard.
    assert!(
        script.contains("eval \"$(command clone shell-init zsh)\""),
        "emitted script should carry the install line with 'command clone shell-init zsh'; got:\n{script}"
    );
    assert!(
        script.contains("hash clone"),
        "emitted script should carry the hash guard; got:\n{script}"
    );
}

#[test]
fn zsh_script_has_shell_init_in_passthrough() {
    let script = init_script("zsh").expect("zsh should be supported");
    // shell-init must appear in the passthrough test alongside -h/--help/-v/--version.
    assert!(
        script.contains("shell-init"),
        "emitted script should include 'shell-init' in the passthrough guard; got:\n{script}"
    );
}

#[test]
fn zsh_script_validates_destination_before_cd() {
    let script = init_script("zsh").expect("zsh should be supported");
    // Destination guard must be present.
    assert!(
        script.contains("! -d \"$dest\""),
        "emitted script should validate destination with '! -d'; got:\n{script}"
    );
    assert!(
        script.contains("-z \"$dest\""),
        "emitted script should validate destination with '-z'; got:\n{script}"
    );
}

#[test]
fn zsh_script_carries_version_marker_in_header() {
    let script = init_script("zsh").expect("zsh should be supported");
    // The header comment must contain the GIT_DESCRIBE placeholder expansion.
    assert!(
        script.contains("[shell-init "),
        "emitted script header should carry the version marker '[shell-init <version>]'; got:\n{script}"
    );
}

// ---- init_script: syntax check via `zsh -n` ----------------------------

#[test]
fn zsh_script_passes_syntax_check() {
    use std::process::Command;
    // Skip if zsh is not on PATH rather than failing CI that lacks zsh.
    let has_zsh = Command::new("zsh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !has_zsh {
        eprintln!("zsh not found on PATH; skipping syntax check");
        return;
    }

    let script = init_script("zsh").expect("zsh should be supported");

    let output = Command::new("zsh")
        .args(["-n", "-c", &script])
        .output()
        .expect("failed to run zsh -n");

    assert!(
        output.status.success(),
        "zsh -n rejected the emitted script:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ---- init_script: unsupported shell -------------------------------------

#[test]
fn unknown_shell_is_rejected() {
    let err = init_script("fish").expect_err("fish should not be supported");
    let msg = err.to_string();
    assert!(msg.contains("clone"), "error should name the command; got: {msg}");
    assert!(msg.contains("fish"), "error should echo the bad shell; got: {msg}");
    assert!(msg.contains("zsh"), "error should list the supported shell; got: {msg}");
}

#[test]
fn bash_shell_is_rejected() {
    let err = init_script("bash").expect_err("bash should not be supported yet");
    let msg = err.to_string();
    assert!(msg.contains("clone"), "error should name the command");
    assert!(msg.contains("bash"), "error should echo the bad shell");
}
