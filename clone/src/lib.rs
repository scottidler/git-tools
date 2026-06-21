// clone — core logic. The binary (`main.rs`) is a thin shell over `run`.

pub mod cli;
pub mod config;

pub use cli::Cli;
pub use config::Config;

use std::path::{Path, PathBuf};

use common::git;
use eyre::{Result, WrapErr, eyre};
use log::debug;

/// Transport URLs tried in order: SSH first, then HTTPS as a fallback.
pub const REMOTE_URLS: [&str; 2] = ["ssh://git@github.com", "https://github.com"];

/// Clone (or update) the repository described by `config`, returning the
/// destination path the shell wrapper should `cd` into.
pub fn run(config: Config) -> Result<PathBuf> {
    let repospec = config.spec.to_string();
    debug!("run: repospec={} clonepath={:?}", repospec, config.clonepath);

    let full_clone_path = config.clonepath.join(&repospec);

    let dest = if full_clone_path.exists() && full_clone_path.read_dir()?.next().is_some() {
        update_existing_repo(&full_clone_path, &config.revision)?;
        full_clone_path
    } else {
        clone_new_repo(&config, &repospec)?
    };

    Ok(dest)
}

fn update_existing_repo(repo: &Path, revision: &str) -> Result<()> {
    debug!("update_existing_repo: repo={:?} revision={}", repo, revision);

    // Check if repo has any commits (handles empty repos from failed clones)
    let head_ok = git::output(&["rev-parse", "HEAD"], Some(repo), None)
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !head_ok {
        // No commits - try to fetch from remote and reset to remote branch
        if git::run(&["fetch", "origin"], Some(repo), None).is_err() {
            return Err(eyre!(
                "Repository exists but has no commits and fetch failed. \
                 Remove {} and try again.",
                repo.display()
            ));
        }

        // Reset local branch to remote (fixes empty repo from failed clone)
        if git::run(&["reset", "--hard", "origin/HEAD"], Some(repo), None).is_err() {
            return Err(eyre!(
                "Repository exists but failed to reset to remote. \
                 Remove {} and try again.",
                repo.display()
            ));
        }

        // Successfully recovered - skip the checkout/pull below, just return
        return Ok(());
    }

    // Check for untracked files
    let status_str = git::output(&["status", "--porcelain"], Some(repo), None)
        .wrap_err("Failed to check git status")?
        .stdout;
    let has_untracked = status_str.lines().any(|line| line.starts_with("??"));

    if has_untracked {
        return Err(eyre!(
            "Cannot update repository: untracked files present.\n\
             Please commit, remove, or add them to .gitignore before cloning.\n\
             Untracked files:\n{}",
            status_str
                .lines()
                .filter(|line| line.starts_with("??"))
                .map(|line| line.trim_start_matches("?? "))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }

    // Check for uncommitted changes and stash them
    let has_changes = !status_str.is_empty();
    if has_changes {
        git::run(
            &["stash", "push", "-m", "Automatic stash by clone tool"],
            Some(repo),
            None,
        )?;
        eprintln!("Note: Uncommitted changes have been stashed. Use 'git stash pop' to restore them.");
    }

    git::run(&["checkout", revision], Some(repo), None)?;
    git::run(&["pull"], Some(repo), None)?;
    Ok(())
}

fn clone_new_repo(config: &Config, repospec: &str) -> Result<PathBuf> {
    debug!("clone_new_repo: repospec={} versioning={}", repospec, config.versioning);

    let revision = if config.versioning {
        fetch_revision_sha(&config.remote, repospec, config.verbose)?
    } else {
        config.revision.clone()
    };

    let full_clone_path = if config.versioning {
        config.clonepath.join(format!("{}/{}", repospec, revision))
    } else {
        config.clonepath.join(repospec)
    };

    let mirror = config.mirrorpath.as_deref();

    // Perform the clone (with SSH fallback)
    let clone_succeeded = if let Some(key) = config.ssh_key.as_deref() {
        let key = key.to_string_lossy();
        attempt_clone_with_ssh(repospec, &full_clone_path, &config.remote, mirror, &key, config.verbose)?
            || attempt_clone_with_ssh(repospec, &full_clone_path, REMOTE_URLS[1], mirror, &key, config.verbose)?
    } else {
        attempt_clone(repospec, &full_clone_path, &config.remote, mirror, config.verbose)?
            || attempt_clone(repospec, &full_clone_path, REMOTE_URLS[1], mirror, config.verbose)?
    };

    if !clone_succeeded {
        return Err(eyre!(
            "Failed to clone repository '{}' from both '{}' and '{}'",
            repospec,
            config.remote,
            REMOTE_URLS[1]
        ));
    }

    // Verify clone actually fetched commits (handles partial clone failures)
    let head_ok = git::output(&["rev-parse", "HEAD"], Some(&full_clone_path), None)
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !head_ok {
        // Clone left an empty repo - clean up and return error
        std::fs::remove_dir_all(&full_clone_path).ok();
        return Err(eyre!(
            "Clone appeared to succeed but repository has no commits. \
             This can happen with newly created repos - please try again."
        ));
    }

    // Checkout requested revision and clean workspace
    git::run(&["checkout", &revision], Some(&full_clone_path), None)?;
    git::run(&["clean", "-xfd"], Some(&full_clone_path), None)?;

    Ok(full_clone_path)
}

fn fetch_revision_sha(remote_url: &str, repospec: &str, _verbose: bool) -> Result<String> {
    let separator = if remote_url.starts_with("git@") { ":" } else { "/" };
    let repo_url = format!("{}{}{}", remote_url, separator, repospec);

    let command_args = ["ls-remote", &repo_url, "HEAD"];
    debug!("Executing git command with args: {:?}", command_args);

    let output = git::output(&command_args, None, None).wrap_err("Failed to execute ls-remote")?;
    if !output.status.success() {
        return Err(eyre!("git ls-remote failed for {}: {}", repo_url, output.stderr.trim()));
    }

    debug!("ls-remote output: {:?}", output.stdout);

    let sha = output
        .stdout
        .lines()
        .filter(|line| line.contains("HEAD"))
        .filter_map(|line| line.split_whitespace().next())
        .next()
        .ok_or_else(|| eyre!("Could not find SHA for HEAD"))
        .map(|s| s.to_string())?;

    Ok(sha)
}

fn attempt_clone_with_ssh(
    repospec: &str,
    full_clone_path: &Path,
    remote_url: &str,
    mirror_option: Option<&Path>,
    ssh_key: &str,
    verbose: bool,
) -> Result<bool> {
    let mut args: Vec<String> = vec![
        "clone".into(),
        format!("{}/{}", remote_url, repospec),
        full_clone_path.to_string_lossy().into_owned(),
    ];
    if let Some(mirror) = mirror_option {
        args.push("--reference".into());
        args.push(format!("{}/{}.git", mirror.display(), repospec));
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = git::run(
        &arg_refs,
        None,
        Some(&[("GIT_SSH_COMMAND", &git::ssh_command(ssh_key))]),
    );

    match result {
        Ok(_) => {
            if verbose {
                eprintln!("Successfully cloned from {} using SSH key {}", remote_url, ssh_key);
            }
            Ok(true)
        }
        Err(e) => {
            if verbose {
                eprintln!("Failed to clone from {} using SSH: {}", remote_url, e);
            }
            Ok(false)
        }
    }
}

fn attempt_clone(
    repospec: &str,
    full_clone_path: &Path,
    remote_url: &str,
    mirror_option: Option<&Path>,
    verbose: bool,
) -> Result<bool> {
    let mut args: Vec<String> = vec![
        "clone".into(),
        format!("{}/{}", remote_url, repospec),
        full_clone_path.to_string_lossy().into_owned(),
    ];
    if let Some(mirror) = mirror_option {
        args.push("--reference".into());
        args.push(format!("{}/{}.git", mirror.display(), repospec));
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = git::run(&arg_refs, None, None);

    match result {
        Ok(_) => {
            if verbose {
                eprintln!("Successfully cloned from {}", remote_url);
            }
            Ok(true)
        }
        Err(e) => {
            if verbose {
                eprintln!("Failed to clone from {}: {}", remote_url, e);
            }
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests;
