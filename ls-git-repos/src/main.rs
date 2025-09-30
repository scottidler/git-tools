use clap::Parser;
use eyre::{eyre, Result};
use log::debug;
use std::path::{Path, PathBuf};
use tokio;
use shellexpand;
use ini::Ini;
use walkdir::WalkDir;
use regex::Regex;

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[command(name = "ls-git-repos", about = "List all local Git repositories with their GitHub reposlug")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = false)]
struct Cli {
    #[clap(value_parser, default_value = ".")]
    path: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    let expanded_path = shellexpand::tilde(&args.path).to_string();
    let base_path = PathBuf::from(expanded_path);

    if !base_path.exists() {
        return Err(eyre!("The specified path does not exist: {}", base_path.display()));
    }

    let reposlugs = find_git_repos(&base_path)?;
    for reposlug in reposlugs {
        println!("{}", reposlug);
    }

    Ok(())
}

/// Recursively finds `.git/config` files and extracts reposlug
fn find_git_repos(base_path: &Path) -> Result<Vec<String>> {
    let mut reposlugs = Vec::new();

    for entry in WalkDir::new(base_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "config" && e.path().parent().map_or(false, |p| p.ends_with(".git")))
    {
        let config_path = entry.path();
        debug!("Found Git config: {}", config_path.display());

        if let Some(reposlug) = parse_git_config(config_path)? {
            reposlugs.push(reposlug);
        }
    }

    reposlugs.sort();
    Ok(reposlugs)
}

/// Parses `.git/config` to extract the repository slug in the format `User/RepoName`
fn parse_git_config(config_path: &Path) -> Result<Option<String>> {
    let config = Ini::load_from_file(config_path)
        .map_err(|e| eyre!("Failed to read Git config file {}: {}", config_path.display(), e))?;

    if let Some(remote) = config.section(Some("remote \"origin\"")) {
        if let Some(url) = remote.get("url") {
            debug!("Extracted remote URL: {}", url);
            return Ok(parse_git_url(url));
        }
    }

    Ok(None)
}

/// Parses a Git remote URL into `User/RepoName`
fn parse_git_url(url: &str) -> Option<String> {
    let https_regex = Regex::new(r"^https://github\.com/([^/]+)/([^/]+)(\.git)?$").unwrap();
    let ssh_regex = Regex::new(r"^git@github\.com:([^/]+)/([^/]+)(\.git)?$").unwrap();

    if let Some(captures) = https_regex.captures(url) {
        let user = &captures[1];
        let repo = &captures[2];
        return Some(format!("{}/{}", user, repo));
    }

    if let Some(captures) = ssh_regex.captures(url) {
        let user = &captures[1];
        let repo = &captures[2];
        return Some(format!("{}/{}", user, repo));
    }

    None
}
