// common — git clone transport: SSH-key-first with HTTPS fallback. Shared by
// `clone` (flat + bare acquisition) and `worktree` (`init`/`migrate`).

use std::path::Path;

use crate::git;
use eyre::{Result, eyre};
use log::debug;

/// Transport URLs tried in order: SSH first, then HTTPS as a fallback.
pub const REMOTE_URLS: [&str; 2] = ["ssh://git@github.com", "https://github.com"];

/// Clone `<remote>/<repospec>` into `target`, trying the primary remote (SSH)
/// first and falling back to HTTPS. When `ssh_key` is present it drives
/// `GIT_SSH_COMMAND` for the attempt. `extra` carries extra `git clone` flags
/// (e.g. `--bare`); `mirror` adds `--reference <mirror>/<repospec>.git`.
///
/// This is the single clone primitive both the flat and bare paths use.
pub fn clone_with_fallback(
    repospec: &str,
    target: &Path,
    primary_remote: &str,
    mirror: Option<&Path>,
    ssh_key: Option<&Path>,
    extra: &[&str],
    verbose: bool,
) -> Result<()> {
    debug!(
        "clone_with_fallback: repospec={} target={:?} primary_remote={} extra={:?}",
        repospec, target, primary_remote, extra
    );

    if try_clone(repospec, target, primary_remote, mirror, ssh_key, extra, verbose)
        || try_clone(repospec, target, REMOTE_URLS[1], mirror, ssh_key, extra, verbose)
    {
        return Ok(());
    }

    Err(eyre!(
        "Failed to clone repository '{}' from both '{}' and '{}'",
        repospec,
        primary_remote,
        REMOTE_URLS[1]
    ))
}

/// Attempt one `git clone` against a single remote. Returns `true` on success.
/// A failure is logged (in verbose mode) and reported as `false` so the caller
/// can fall back to the next remote.
fn try_clone(
    repospec: &str,
    target: &Path,
    remote_url: &str,
    mirror: Option<&Path>,
    ssh_key: Option<&Path>,
    extra: &[&str],
    verbose: bool,
) -> bool {
    let mut args: Vec<String> = vec!["clone".into()];
    for flag in extra {
        args.push((*flag).to_string());
    }
    args.push(format!("{}/{}", remote_url, repospec));
    args.push(target.to_string_lossy().into_owned());
    if let Some(mirror) = mirror {
        args.push("--reference".into());
        args.push(format!("{}/{}.git", mirror.display(), repospec));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let envs = ssh_key.map(|k| [("GIT_SSH_COMMAND".to_string(), git::ssh_command(&k.to_string_lossy()))]);
    let env_refs: Option<Vec<(&str, &str)>> = envs
        .as_ref()
        .map(|e| e.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect());

    match git::run(&arg_refs, None, env_refs.as_deref()) {
        Ok(_) => {
            if verbose {
                match ssh_key {
                    Some(key) => {
                        eprintln!(
                            "Successfully cloned from {} using SSH key {}",
                            remote_url,
                            key.display()
                        )
                    }
                    None => eprintln!("Successfully cloned from {}", remote_url),
                }
            }
            true
        }
        Err(e) => {
            if verbose {
                eprintln!("Failed to clone from {}: {}", remote_url, e);
            }
            false
        }
    }
}

#[cfg(test)]
mod tests;
