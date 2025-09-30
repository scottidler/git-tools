// clone

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::Parser;
use eyre::{Result, eyre, WrapErr};
use log::{debug, warn};
use env_logger;
use ini::ini;

const REMOTE_URLS: [&str; 2] = [
    "ssh://git@github.com",
    "https://github.com",
];

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[arg(help = "repospec schema is remote?reponame", required = true)]
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

    let full_clone_path = PathBuf::from(&cli.clonepath).join(&cli.repospec);

    if full_clone_path.exists() && full_clone_path.read_dir()?.next().is_some() {
        update_existing_repo(&full_clone_path, &cli.revision)?
    } else {
        clone_new_repo(&cli)?
    }

    println!("{}", cli.repospec);

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
    if status.success() { Ok(()) } else { Err(eyre!("git {:?} exited {}", args, status)) }
}

fn update_existing_repo(full_clone_path: &Path, revision: &str) -> Result<()> {
    std::env::set_current_dir(full_clone_path)
        .wrap_err("Failed to set current directory")?;

    git(&["checkout", revision], None)?;
    git(&["pull"], None)?;
    git(&["clean", "-xfd"], None)?;
    Ok(())
}

fn clone_new_repo(cli: &Cli) -> Result<()> {
    let revision = if cli.versioning {
        fetch_revision_sha(&cli.remote, &cli.repospec, cli.verbose)?
    } else {
        cli.revision.clone()
    };

    let full_clone_path = if cli.versioning {
        PathBuf::from(&cli.clonepath).join(format!("{}/{}", cli.repospec, revision))
    } else {
        PathBuf::from(&cli.clonepath).join(&cli.repospec)
    };

    // Perform the clone (with SSH fallback)
    if let Some(key) = find_ssh_key_for_org(&cli.repospec)? {
        if !attempt_clone_with_ssh(&cli.repospec, &full_clone_path, &cli.remote, &cli.mirrorpath, &key, cli.verbose)? {
            attempt_clone_with_ssh(&cli.repospec, &full_clone_path, REMOTE_URLS[1], &cli.mirrorpath, &key, cli.verbose)?;
        }
    } else {
        if !attempt_clone(&cli.repospec, &full_clone_path, &cli.remote, &cli.mirrorpath, cli.verbose)? {
            attempt_clone(&cli.repospec, &full_clone_path, REMOTE_URLS[1], &cli.mirrorpath, cli.verbose)?;
        }
    }

    // Change into the new repository directory
    std::env::set_current_dir(&full_clone_path)
        .wrap_err("Failed to change directory into cloned repo")?;

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
        .args(&command_args)
        .stdout(Stdio::null())
        .output()
        .wrap_err("Failed to execute ls-remote")?;

    debug!("ls-remote output: {:?}", String::from_utf8_lossy(&output.stdout));

    let output_str = String::from_utf8(output.stdout).wrap_err("Failed to parse ls-remote output")?;
    let sha = output_str.lines()
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
    _verbose: bool,
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
    git(&arg_refs, Some(&[("GIT_SSH_COMMAND", &format!("/usr/bin/ssh -i {}", ssh_key))]))
        .map(|_| true)
        .or(Ok(false))
}

fn attempt_clone(
    repospec: &str,
    full_clone_path: &Path,
    remote_url: &str,
    mirror_option: &Option<String>,
    _verbose: bool,
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
    git(&arg_refs, None).map(|_| true).or(Ok(false))
}

fn find_ssh_key_for_org(repospec: &str) -> Result<Option<String>> {
    let home_dir = env::var("HOME").wrap_err("Failed to get HOME environment variable")?;
    let config_path = env::var("CLONE_CFG")
        .unwrap_or_else(|_| format!("{}/.config/clone/clone.cfg", home_dir));

    if !Path::new(&config_path).exists() {
        warn!("Configuration file not found: {:?}", config_path);
        return Ok(None);
    }

    let cfg = ini!(&config_path);
    if cfg.is_empty() {
        return Err(eyre!("Failed to load configuration file"));
    }

    let org_name = repospec.split('/').next().ok_or_else(|| eyre!("Invalid repospec format"))?;
    let section_key = format!("org.{}", org_name);
    let ssh_key_map = cfg.get(&section_key).or_else(|| cfg.get("org.default"))
        .ok_or_else(|| eyre!("Configuration section not found"))?;

    let ssh_key = ssh_key_map.get("sshkey").cloned().flatten();

    Ok(ssh_key)
}
