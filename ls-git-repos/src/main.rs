use clap::Parser;
use common::language::{detect_language, matches_language};
use eyre::{eyre, Result};
use ini::Ini;
use log::debug;
use rayon::prelude::*;
use regex::Regex;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[command(
    name = "ls-git-repos",
    about = "List all local Git repositories with their GitHub reposlug"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = false)]
struct Cli {
    #[clap(value_parser, default_value = ".")]
    path: String,

    #[clap(short, long, num_args = 1..)]
    lang: Vec<String>,
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

    let repos = find_git_repos(&base_path)?;

    let mut results: Vec<String> = if args.lang.is_empty() {
        repos.into_iter().map(|(slug, _)| slug).collect()
    } else {
        repos
            .par_iter()
            .filter_map(|(slug, path)| {
                let detected = detect_language(path);
                if matches_language(detected.as_deref(), &args.lang) {
                    Some(slug.clone())
                } else {
                    None
                }
            })
            .collect()
    };

    results.sort();
    for slug in results {
        println!("{}", slug);
    }

    Ok(())
}

/// Recursively finds `.git/config` files and extracts (reposlug, repo_root) pairs
fn find_git_repos(base_path: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut repos = Vec::new();

    for entry in WalkDir::new(base_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "config" && e.path().parent().is_some_and(|p| p.ends_with(".git")))
    {
        let config_path = entry.path();
        debug!("Found Git config: {}", config_path.display());

        if let Some(slug) = parse_git_config(config_path)? {
            // repo root is parent of .git/, which is parent of config
            if let Some(repo_root) = config_path.parent().and_then(|git_dir| git_dir.parent()) {
                repos.push((slug, repo_root.to_path_buf()));
            }
        }
    }

    Ok(repos)
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
