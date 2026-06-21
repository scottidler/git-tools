// clone — core logic. The binary (`main.rs`) is a thin shell over `run`.

pub mod bare;
pub mod cli;
pub mod config;
pub mod transport;
pub mod worktree;

pub use cli::Cli;
pub use config::{Config, Layout, Op};

use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use log::debug;

/// Transport URLs tried in order: SSH first, then HTTPS as a fallback.
pub const REMOTE_URLS: [&str; 2] = ["ssh://git@github.com", "https://github.com"];

/// Execute the operation described by `config`, returning the destination path
/// the shell wrapper should `cd` into.
pub fn run(config: Config) -> Result<PathBuf> {
    match &config.op {
        Op::Clone => {
            let spec = config
                .spec
                .clone()
                .expect("Op::Clone requires a spec (enforced in Config::try_from)");
            run_clone(&config, &spec)
        }
        Op::AddWorktree(branch) => worktree::add(&config, branch),
    }
}

/// Clone (or update) `spec`, dispatching on the resolved layout.
fn run_clone(config: &Config, spec: &RepoSpec) -> Result<PathBuf> {
    let repospec = spec.to_string();
    debug!(
        "run_clone: repospec={} clonepath={:?} layout={:?}",
        repospec, config.clonepath, config.layout
    );

    let target = config.clonepath.join(&repospec);

    match config.layout {
        Layout::Flat => clone_or_update_flat(config, &repospec, &target),
        Layout::Bare => run_bare(config, spec, &repospec, &target),
    }
}

/// Bare-layout dispatch, handling the mixed-ecosystem cases the default flip
/// introduces:
/// - an existing bare container → idempotent reconcile;
/// - an existing *flat* checkout (not yet migrated) → update it in place and
///   hint at `--migrate`, never silently convert;
/// - otherwise → set up a fresh bare container.
fn run_bare(config: &Config, spec: &RepoSpec, repospec: &str, target: &Path) -> Result<PathBuf> {
    if bare::is_bare_container(target) {
        return bare::reconcile_container(config, target);
    }

    if is_flat_clone(target) {
        let dest = update_existing_repo(target, &config.revision).map(|_| target.to_path_buf())?;
        eprintln!(
            "Note: '{}' is a flat checkout. Run `clone --migrate {}` to convert it to the bare-worktree layout.",
            target.display(),
            repospec
        );
        return Ok(dest);
    }

    bare::setup_bare_container(config, spec)
}

/// Flat-layout: update an existing non-empty checkout, else fresh flat clone.
fn clone_or_update_flat(config: &Config, repospec: &str, target: &Path) -> Result<PathBuf> {
    if target.exists() && target.read_dir()?.next().is_some() {
        update_existing_repo(target, &config.revision)?;
        Ok(target.to_path_buf())
    } else {
        clone_new_repo(config, repospec)
    }
}

/// Whether `path` is a flat checkout (`.git` dir or file) that is *not* a bare
/// container.
fn is_flat_clone(path: &Path) -> bool {
    let git = path.join(".git");
    (git.is_dir() || git.is_file()) && !bare::is_bare_container(path)
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

    // Perform the clone (SSH key first, HTTPS fallback).
    transport::clone_with_fallback(
        repospec,
        &full_clone_path,
        &config.remote,
        config.mirrorpath.as_deref(),
        config.ssh_key.as_deref(),
        &[],
        config.verbose,
    )?;

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

#[cfg(test)]
mod tests;
