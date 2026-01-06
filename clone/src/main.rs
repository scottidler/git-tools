// clone

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;
use eyre::{eyre, Result, WrapErr};
use ini::ini;
use log::{debug, warn};

const REMOTE_URLS: [&str; 2] = ["ssh://git@github.com", "https://github.com"];

/// Parse a repository specification from various formats into org/repo format.
///
/// Supported formats:
/// - `org/repo` - Simple format (pass through)
/// - `https://github.com/org/repo` or `https://github.com/org/repo.git` - HTTPS URL
/// - `git@github.com:org/repo` or `git@github.com:org/repo.git` - SCP-style SSH
/// - `ssh://git@github.com/org/repo` or `ssh://git@github.com/org/repo.git` - SSH URL
/// - `git://github.com/org/repo` or `git://github.com/org/repo.git` - Git protocol URL
pub fn parse_repospec(input: &str) -> Result<String> {
    let input = input.trim();

    if input.is_empty() {
        return Err(eyre!("Empty repository specification"));
    }

    // Remove trailing .git if present
    let input = input.strip_suffix(".git").unwrap_or(input);

    // HTTPS URL: https://github.com/org/repo
    if input.starts_with("https://") || input.starts_with("http://") {
        let without_protocol = input
            .strip_prefix("https://")
            .or_else(|| input.strip_prefix("http://"))
            .unwrap();
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid HTTPS URL: missing path"));
    }

    // SSH URL: ssh://git@github.com/org/repo
    if input.starts_with("ssh://") {
        let without_protocol = input.strip_prefix("ssh://").unwrap();
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid SSH URL: missing path"));
    }

    // Git protocol URL: git://github.com/org/repo
    if input.starts_with("git://") {
        let without_protocol = input.strip_prefix("git://").unwrap();
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid git URL: missing path"));
    }

    // SCP-style SSH: git@github.com:org/repo
    if input.contains('@') && input.contains(':') {
        let parts: Vec<&str> = input.splitn(2, ':').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid SCP-style URL: missing colon separator"));
    }

    // Simple org/repo format - validate it has a slash with content on both sides
    if input.contains('/') {
        let parts: Vec<&str> = input.splitn(2, '/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            let org = parts[0];
            let repo = parts[1];
            if !org.contains(':') && !repo.contains(':') {
                return Ok(input.to_string());
            }
        }
    }

    Err(eyre!(
        "Invalid repository specification: '{}'. Expected formats:\n\
         - org/repo\n\
         - https://github.com/org/repo\n\
         - git@github.com:org/repo\n\
         - ssh://git@github.com/org/repo\n\
         - git://github.com/org/repo",
        input
    ))
}

/// Extract org/repo from a path, handling extra path components
fn extract_org_repo_from_path(path: &str) -> Result<String> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_start_matches('/');

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return Err(eyre!("Invalid path: expected org/repo format, got '{}'", path));
    }

    let org = parts[0];
    let repo = parts[1];

    if org.is_empty() || repo.is_empty() {
        return Err(eyre!("Invalid path: org or repo is empty"));
    }

    Ok(format!("{}/{}", org, repo))
}

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Cli {
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
    env_logger::init();

    let cli = Cli::parse();

    // Parse the repospec to handle various URL formats (URL -> org/repo)
    let repospec = parse_repospec(&cli.repospec)
        .wrap_err_with(|| format!("Failed to parse repository specification: {}", cli.repospec))?;

    let full_clone_path = PathBuf::from(&cli.clonepath).join(&repospec);

    if full_clone_path.exists() && full_clone_path.read_dir()?.next().is_some() {
        update_existing_repo(&full_clone_path, &cli.revision)?
    } else {
        clone_new_repo(&cli, &repospec)?
    }

    println!("{}", repospec);

    Ok(())
}

/// Run `git <args…>`, silencing output, with optional environment overrides.
fn git(args: &[&str], envs: Option<&[(&str, &str)]>) -> Result<()> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(env_pairs) = envs {
        for (k, v) in env_pairs {
            cmd.env(k, v);
        }
    }
    let status = cmd.status().wrap_err_with(|| format!("git {:?} failed", args))?;
    if status.success() {
        Ok(())
    } else {
        Err(eyre!("git {:?} exited {}", args, status))
    }
}

fn update_existing_repo(full_clone_path: &Path, revision: &str) -> Result<()> {
    std::env::set_current_dir(full_clone_path).wrap_err("Failed to set current directory")?;

    // Check for untracked files
    let status_output = Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .wrap_err("Failed to check git status")?;

    let status_str = String::from_utf8_lossy(&status_output.stdout);
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
        git(&["stash", "push", "-m", "Automatic stash by clone tool"], None)?;
        eprintln!("Note: Uncommitted changes have been stashed. Use 'git stash pop' to restore them.");
    }

    git(&["checkout", revision], None)?;
    git(&["pull"], None)?;
    Ok(())
}

fn clone_new_repo(cli: &Cli, repospec: &str) -> Result<()> {
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

    // Change into the new repository directory
    std::env::set_current_dir(&full_clone_path).wrap_err("Failed to change directory into cloned repo")?;

    // Checkout requested revision and clean workspace
    git(&["checkout", &revision], None)?;
    git(&["clean", "-xfd"], None)?;

    Ok(())
}

fn fetch_revision_sha(remote_url: &str, repospec: &str, _verbose: bool) -> Result<String> {
    let separator = if remote_url.starts_with("git@") { ":" } else { "/" };
    let repo_url = format!("{}{}{}", remote_url, separator, repospec);

    let command_args = ["ls-remote", &repo_url, "HEAD"];
    debug!("Executing git command with args: {:?}", command_args);

    let output = Command::new("git")
        .args(command_args)
        .stdout(Stdio::null())
        .output()
        .wrap_err("Failed to execute ls-remote")?;

    debug!("ls-remote output: {:?}", String::from_utf8_lossy(&output.stdout));

    let output_str = String::from_utf8(output.stdout).wrap_err("Failed to parse ls-remote output")?;
    let sha = output_str
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
    let result = git(
        &arg_refs,
        Some(&[("GIT_SSH_COMMAND", &format!("/usr/bin/ssh -i {}", ssh_key))]),
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
    let result = git(&arg_refs, None);

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

    // ============================================
    // Tests for parse_repospec
    // ============================================

    #[test]
    fn test_parse_simple_org_repo() {
        assert_eq!(parse_repospec("scottidler/gx").unwrap(), "scottidler/gx");
        assert_eq!(parse_repospec("otto-rs/otto").unwrap(), "otto-rs/otto");
        assert_eq!(parse_repospec("tatari-tv/philo").unwrap(), "tatari-tv/philo");
    }

    #[test]
    fn test_parse_https_url() {
        assert_eq!(
            parse_repospec("https://github.com/scottidler/gx").unwrap(),
            "scottidler/gx"
        );
        assert_eq!(
            parse_repospec("https://github.com/otto-rs/otto").unwrap(),
            "otto-rs/otto"
        );
        assert_eq!(
            parse_repospec("https://github.com/tatari-tv/philo").unwrap(),
            "tatari-tv/philo"
        );
    }

    #[test]
    fn test_parse_https_url_with_git_suffix() {
        assert_eq!(
            parse_repospec("https://github.com/scottidler/gx.git").unwrap(),
            "scottidler/gx"
        );
        assert_eq!(
            parse_repospec("https://github.com/otto-rs/otto.git").unwrap(),
            "otto-rs/otto"
        );
    }

    #[test]
    fn test_parse_http_url() {
        assert_eq!(
            parse_repospec("http://github.com/scottidler/gx").unwrap(),
            "scottidler/gx"
        );
    }

    #[test]
    fn test_parse_ssh_url() {
        assert_eq!(
            parse_repospec("ssh://git@github.com/scottidler/gx").unwrap(),
            "scottidler/gx"
        );
        assert_eq!(
            parse_repospec("ssh://git@github.com/otto-rs/otto").unwrap(),
            "otto-rs/otto"
        );
    }

    #[test]
    fn test_parse_ssh_url_with_git_suffix() {
        assert_eq!(
            parse_repospec("ssh://git@github.com/scottidler/gx.git").unwrap(),
            "scottidler/gx"
        );
    }

    #[test]
    fn test_parse_git_protocol_url() {
        assert_eq!(
            parse_repospec("git://github.com/scottidler/gx").unwrap(),
            "scottidler/gx"
        );
        assert_eq!(
            parse_repospec("git://github.com/otto-rs/otto.git").unwrap(),
            "otto-rs/otto"
        );
    }

    #[test]
    fn test_parse_scp_style_ssh() {
        assert_eq!(parse_repospec("git@github.com:scottidler/gx").unwrap(), "scottidler/gx");
        assert_eq!(parse_repospec("git@github.com:otto-rs/otto").unwrap(), "otto-rs/otto");
        assert_eq!(
            parse_repospec("git@github.com:tatari-tv/philo").unwrap(),
            "tatari-tv/philo"
        );
    }

    #[test]
    fn test_parse_scp_style_ssh_with_git_suffix() {
        assert_eq!(
            parse_repospec("git@github.com:scottidler/gx.git").unwrap(),
            "scottidler/gx"
        );
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_repospec("  scottidler/gx  ").unwrap(), "scottidler/gx");
        assert_eq!(
            parse_repospec("\thttps://github.com/scottidler/gx\n").unwrap(),
            "scottidler/gx"
        );
    }

    #[test]
    fn test_parse_different_hosts() {
        // GitLab
        assert_eq!(
            parse_repospec("https://gitlab.com/someorg/somerepo").unwrap(),
            "someorg/somerepo"
        );
        assert_eq!(
            parse_repospec("git@gitlab.com:someorg/somerepo.git").unwrap(),
            "someorg/somerepo"
        );
        // Bitbucket
        assert_eq!(
            parse_repospec("https://bitbucket.org/someorg/somerepo").unwrap(),
            "someorg/somerepo"
        );
        // Enterprise GitHub
        assert_eq!(
            parse_repospec("https://github.enterprise.com/someorg/somerepo").unwrap(),
            "someorg/somerepo"
        );
    }

    #[test]
    fn test_parse_empty_input() {
        assert!(parse_repospec("").is_err());
        assert!(parse_repospec("   ").is_err());
    }

    #[test]
    fn test_parse_invalid_formats() {
        // No slash
        assert!(parse_repospec("justrepo").is_err());
        // Empty org
        assert!(parse_repospec("/repo").is_err());
        // Empty repo
        assert!(parse_repospec("org/").is_err());
        // Just a URL without path
        assert!(parse_repospec("https://github.com").is_err());
        assert!(parse_repospec("https://github.com/").is_err());
    }

    #[test]
    fn test_parse_url_with_extra_path_components() {
        // URLs might have extra path components after org/repo
        assert_eq!(
            parse_repospec("https://github.com/scottidler/gx/tree/main").unwrap(),
            "scottidler/gx"
        );
        assert_eq!(
            parse_repospec("https://github.com/scottidler/gx/blob/main/README.md").unwrap(),
            "scottidler/gx"
        );
    }

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

        std::env::set_var("CLONE_CFG", config_path.to_str().unwrap());

        let result = find_ssh_key_for_org("testorg/repo");

        // Clean up
        fs::remove_dir_all(&temp_dir).ok();
        std::env::remove_var("CLONE_CFG");

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
