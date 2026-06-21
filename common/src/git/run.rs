use std::path::Path;
use std::process::{Command, ExitStatus};

use eyre::{Result, WrapErr, eyre};
use log::debug;

/// Captured output of a git invocation.
#[derive(Debug, Clone)]
pub struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: ExitStatus,
}

/// Build a `git` command with optional working directory and env overrides.
fn build(args: &[&str], cwd: Option<&Path>, envs: Option<&[(&str, &str)]>) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if let Some(pairs) = envs {
        for (k, v) in pairs {
            cmd.env(k, v);
        }
    }
    cmd
}

/// Run `git <args…>`, capturing stderr; a non-zero exit is an `Err` carrying
/// the captured stderr in its context. Both stdout and stderr are captured (and
/// discarded on success), matching the old silent behavior while giving callers
/// a diagnosable error. `envs` carries overrides such as `GIT_SSH_COMMAND`.
pub fn run(args: &[&str], cwd: Option<&Path>, envs: Option<&[(&str, &str)]>) -> Result<()> {
    debug!("git::run: args={:?} cwd={:?}", args, cwd);
    let out = build(args, cwd, envs)
        .output()
        .wrap_err_with(|| format!("failed to execute git {:?}", args))?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(eyre!("git {:?} exited {}: {}", args, out.status, stderr.trim()))
    }
}

/// Run `git <args…>`, capturing both pipes. A non-zero exit is **not** an error;
/// the caller inspects `.status`. `envs` is present for read commands that hit
/// the network (e.g. `ls-remote` in versioning mode needs the SSH key).
pub fn output(args: &[&str], cwd: Option<&Path>, envs: Option<&[(&str, &str)]>) -> Result<GitOutput> {
    debug!("git::output: args={:?} cwd={:?}", args, cwd);
    let out = build(args, cwd, envs)
        .output()
        .wrap_err_with(|| format!("failed to execute git {:?}", args))?;
    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        status: out.status,
    })
}

/// POSIX single-quote a string so it survives the shell intact (handles spaces
/// and embedded single quotes).
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the `GIT_SSH_COMMAND` value for a per-org SSH key, shell-quoting the
/// key path so a path containing spaces no longer breaks the command (the bug
/// in clone's former `format!("/usr/bin/ssh -i {}", key)`).
pub fn ssh_command(key_path: &str) -> String {
    format!("/usr/bin/ssh -i {}", shell_quote(key_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_captures_stdout() {
        let out = output(&["--version"], None, None).unwrap();
        assert!(out.status.success());
        assert!(out.stdout.contains("git version"));
    }

    #[test]
    fn test_run_ok_on_success() {
        assert!(run(&["--version"], None, None).is_ok());
    }

    #[test]
    fn test_run_err_carries_stderr() {
        // A bogus subcommand exits non-zero and writes to stderr.
        let err = run(&["definitely-not-a-git-command"], None, None).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("git"));
    }

    #[test]
    fn test_output_nonzero_is_not_error() {
        let out = output(&["rev-parse", "--verify", "refs/heads/no-such-branch-xyz"], None, None).unwrap();
        assert!(!out.status.success());
    }

    #[test]
    fn test_shell_quote() {
        assert_eq!(shell_quote("/home/user/key"), "'/home/user/key'");
        assert_eq!(shell_quote("/path with spaces/key"), "'/path with spaces/key'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn test_ssh_command_quotes_key() {
        assert_eq!(
            ssh_command("/path with spaces/id_ed25519"),
            "/usr/bin/ssh -i '/path with spaces/id_ed25519'"
        );
    }
}
