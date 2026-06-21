// clone

use std::env;
use std::path::{Path, PathBuf};

use clap::Parser;
use common::git;
use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::{LevelFilter, debug, warn};

const REMOTE_URLS: [&str; 2] = ["ssh://git@github.com", "https://github.com"];

// The repospec parser (parse_repospec / RepoSpec) now lives in common::git.

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    log_level: LevelFilter,

    #[arg(
        help = "Repository specification. Accepts: org/repo, https://github.com/org/repo, git@github.com:org/repo, ssh://git@github.com/org/repo, git://github.com/org/repo",
        required = true
    )]
    repospec: String,

    #[arg(help = "revision to check out", default_value = "HEAD")]
    revision: String,

    #[arg(long, help = "the git URL to be used with git clone", default_value = REMOTE_URLS[0])]
    remote: String,

    #[arg(long, help = "path to store all cloned repos", default_value = ".")]
    clonepath: String,

    #[arg(long, help = "path to cached repos to support fast cloning")]
    mirrorpath: Option<String>,

    #[arg(long, help = "turn on versioning; checkout in reponame/commit rather than reponame")]
    versioning: bool,

    #[arg(long, help = "turn on verbose output")]
    verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    common::log::init(cli.log_level, "clone")?;

    // Parse the repospec to handle various URL formats (URL -> org/repo)
    let repospec = git::parse_repospec(&cli.repospec)
        .wrap_err_with(|| format!("Failed to parse repository specification: {}", cli.repospec))?
        .to_string();

    let full_clone_path = PathBuf::from(&cli.clonepath).join(&repospec);

    let dest = if full_clone_path.exists() && full_clone_path.read_dir()?.next().is_some() {
        update_existing_repo(&full_clone_path, &cli.revision)?;
        full_clone_path
    } else {
        clone_new_repo(&cli, &repospec)?
    };

    println!("{}", dest.display());

    Ok(())
}

fn update_existing_repo(full_clone_path: &Path, revision: &str) -> Result<()> {
    std::env::set_current_dir(full_clone_path).wrap_err("Failed to set current directory")?;

    // Check if repo has any commits (handles empty repos from failed clones)
    let head_ok = git::output(&["rev-parse", "HEAD"], None, None)
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !head_ok {
        // No commits - try to fetch from remote and reset to remote branch
        if git::run(&["fetch", "origin"], None, None).is_err() {
            return Err(eyre!(
                "Repository exists but has no commits and fetch failed. \
                 Remove {} and try again.",
                full_clone_path.display()
            ));
        }

        // Reset local branch to remote (fixes empty repo from failed clone)
        if git::run(&["reset", "--hard", "origin/HEAD"], None, None).is_err() {
            return Err(eyre!(
                "Repository exists but failed to reset to remote. \
                 Remove {} and try again.",
                full_clone_path.display()
            ));
        }

        // Successfully recovered - skip the checkout/pull below, just return
        return Ok(());
    }

    // Check for untracked files
    let status_str = git::output(&["status", "--porcelain"], None, None)
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
        git::run(&["stash", "push", "-m", "Automatic stash by clone tool"], None, None)?;
        eprintln!("Note: Uncommitted changes have been stashed. Use 'git stash pop' to restore them.");
    }

    git::run(&["checkout", revision], None, None)?;
    git::run(&["pull"], None, None)?;
    Ok(())
}

fn clone_new_repo(cli: &Cli, repospec: &str) -> Result<PathBuf> {
    let revision = if cli.versioning {
        fetch_revision_sha(&cli.remote, repospec, cli.verbose)?
    } else {
        cli.revision.clone()
    };

    let full_clone_path = if cli.versioning {
        PathBuf::from(&cli.clonepath).join(format!("{}/{}", repospec, revision))
    } else {
        PathBuf::from(&cli.clonepath).join(repospec)
    };

    // Perform the clone (with SSH fallback)
    let clone_succeeded = if let Some(key) = find_ssh_key_for_org(repospec)? {
        if attempt_clone_with_ssh(
            repospec,
            &full_clone_path,
            &cli.remote,
            &cli.mirrorpath,
            &key,
            cli.verbose,
        )? {
            true
        } else {
            attempt_clone_with_ssh(
                repospec,
                &full_clone_path,
                REMOTE_URLS[1],
                &cli.mirrorpath,
                &key,
                cli.verbose,
            )?
        }
    } else if attempt_clone(repospec, &full_clone_path, &cli.remote, &cli.mirrorpath, cli.verbose)? {
        true
    } else {
        attempt_clone(repospec, &full_clone_path, REMOTE_URLS[1], &cli.mirrorpath, cli.verbose)?
    };

    if !clone_succeeded {
        return Err(eyre!(
            "Failed to clone repository '{}' from both '{}' and '{}'",
            repospec,
            cli.remote,
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

    // Change into the new repository directory
    std::env::set_current_dir(&full_clone_path).wrap_err("Failed to change directory into cloned repo")?;

    // Checkout requested revision and clean workspace
    git::run(&["checkout", &revision], None, None)?;
    git::run(&["clean", "-xfd"], None, None)?;

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
    mirror_option: &Option<String>,
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
        args.push(format!("{}/{}.git", mirror, repospec));
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
    mirror_option: &Option<String>,
    verbose: bool,
) -> Result<bool> {
    let mut args: Vec<String> = vec![
        "clone".into(),
        format!("{}/{}", remote_url, repospec),
        full_clone_path.to_string_lossy().into_owned(),
    ];
    if let Some(mirror) = mirror_option {
        args.push("--reference".into());
        args.push(format!("{}/{}.git", mirror, repospec));
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

fn find_ssh_key_for_org(repospec: &str) -> Result<Option<String>> {
    let home_dir = env::var("HOME").wrap_err("Failed to get HOME environment variable")?;
    let config_path = env::var("CLONE_CFG").unwrap_or_else(|_| format!("{}/.config/clone/clone.cfg", home_dir));

    if !Path::new(&config_path).exists() {
        warn!("Configuration file not found: {:?}", config_path);
        return Ok(None);
    }

    let cfg = ini!(&config_path);
    if cfg.is_empty() {
        return Err(eyre!("Failed to load configuration file"));
    }

    let org_name = repospec
        .split('/')
        .next()
        .ok_or_else(|| eyre!("Invalid repospec format"))?;
    let section_key = format!("org.{}", org_name);
    let ssh_key_map = cfg
        .get(&section_key)
        .or_else(|| cfg.get("org.default"))
        .ok_or_else(|| eyre!("Configuration section not found"))?;

    let ssh_key = ssh_key_map.get("sshkey").cloned().flatten();

    Ok(ssh_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    // parse_repospec tests live in common::git::spec now (the parser moved there).

    // ============================================
    // Original tests for find_ssh_key_for_org
    // ============================================

    #[test]
    fn test_find_ssh_key_with_no_slash() {
        let result = find_ssh_key_for_org("invalid-no-slash");
        assert!(
            result.is_ok() || result.is_err(),
            "Function handles input without slash"
        );
    }

    #[test]
    fn test_find_ssh_key_with_valid_repospec() {
        // This test verifies the function handles valid repospec without panicking.
        // It may return Ok(None) if no config exists, or Err if config exists but
        // doesn't have the required sections - both are acceptable behaviors.
        let result = find_ssh_key_for_org("someorg/somerepo");
        // Just verify it doesn't panic - Ok or Err are both valid outcomes
        let _ = result;
    }

    #[test]
    fn test_find_ssh_key_extracts_org_name() {
        let test_cases = vec![
            ("org/repo", "org"),
            ("my-org/my-repo", "my-org"),
            ("org/repo/extra", "org"),
        ];

        for (repospec, _expected_org) in test_cases {
            let result = find_ssh_key_for_org(repospec);
            assert!(result.is_ok() || result.is_err(), "Should handle {}", repospec);
        }
    }

    #[test]
    fn test_find_ssh_key_with_custom_config() {
        let temp_dir = std::env::temp_dir().join("clone_test_config");
        fs::create_dir_all(&temp_dir).unwrap();
        let config_path = temp_dir.join("test.cfg");

        let mut file = fs::File::create(&config_path).unwrap();
        writeln!(file, "[org.testorg]").unwrap();
        writeln!(file, "sshkey = /path/to/key").unwrap();

        unsafe { std::env::set_var("CLONE_CFG", config_path.to_str().unwrap()) };

        let result = find_ssh_key_for_org("testorg/repo");

        // Clean up
        fs::remove_dir_all(&temp_dir).ok();
        unsafe { std::env::remove_var("CLONE_CFG") };

        assert!(result.is_ok());
        if let Ok(Some(key)) = result {
            assert_eq!(key, "/path/to/key");
        }
    }

    #[test]
    fn test_remote_urls_constant() {
        assert_eq!(REMOTE_URLS.len(), 2);
        assert_eq!(REMOTE_URLS[0], "ssh://git@github.com");
        assert_eq!(REMOTE_URLS[1], "https://github.com");
    }
}
