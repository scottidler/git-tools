use super::*;

#[test]
fn unsupported_names_the_command() {
    let err = unsupported("clone", &["zsh"], "fish");
    let msg = err.to_string();
    assert!(
        msg.contains("clone"),
        "error message should name the command; got: {msg}"
    );
}

#[test]
fn unsupported_echoes_the_bad_shell() {
    let err = unsupported("worktree", &["zsh"], "powershell");
    let msg = err.to_string();
    assert!(
        msg.contains("powershell"),
        "error message should echo the bad shell name; got: {msg}"
    );
}

#[test]
fn unsupported_lists_the_supported_set() {
    let err = unsupported("clone", &["zsh", "bash"], "fish");
    let msg = err.to_string();
    assert!(
        msg.contains("zsh"),
        "error message should list supported shell 'zsh'; got: {msg}"
    );
    assert!(
        msg.contains("bash"),
        "error message should list supported shell 'bash'; got: {msg}"
    );
}

#[test]
fn unsupported_single_shell_in_supported_set() {
    let err = unsupported("clone", &["zsh"], "bash");
    let msg = err.to_string();
    assert!(msg.contains("clone"), "should name the command");
    assert!(msg.contains("bash"), "should echo the bad shell");
    assert!(msg.contains("zsh"), "should list the supported shell");
}

#[test]
fn unsupported_empty_supported_set() {
    let err = unsupported("clone", &[], "zsh");
    let msg = err.to_string();
    assert!(msg.contains("clone"), "should name the command");
    assert!(msg.contains("zsh"), "should echo the bad shell");
}
