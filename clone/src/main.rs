// Standard library imports
use std::path::{Path, PathBuf};
use std::process::{Command as SysCommand, Stdio};
use std::env;

// Third-party crate imports
use clap::Parser;
use eyre::{Result, eyre, WrapErr};
use log::{debug, warn, error};
use env_logger;

const REMOTE_URLS: [&str; 2] = [
    "ssh://git@github.com",
    "https://github.com",
];

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/git_describe.rs"));
}

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = built_info::GIT_DESCRIBE)]
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

fn update_existing_repo(full_clone_path: &Path, revision: &str) -> Result<()> {
    env::set_current_dir(full_clone_path)?;
    SysCommand::new("git")
        .args(["checkout", revision])
        .stdout(Stdio::null()) // Suppressing stdout output from the checkout command
        .status()
        .wrap_err("Failed to checkout the specified revision")?;

    SysCommand::new("git")
        .args(["pull"])
        .stdout(Stdio::null()) // Suppressing stdout output from the checkout command
        .status()
        .wrap_err("Failed to pull the latest changes")?;

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

    debug!("Attempting to clone into {:?}", full_clone_path);

    let mirror_option = cli.mirrorpath.as_ref().map(|mirror|
        format!("--reference {}/{}.git", mirror, cli.repospec)
    );

    if !attempt_clone(&cli.repospec, &full_clone_path, &cli.remote, &mirror_option, cli.verbose)? {
        warn!("SSH failed, trying HTTPS...");
        if !attempt_clone(&cli.repospec, &full_clone_path, REMOTE_URLS[1], &mirror_option, cli.verbose)? {
            error!("Failed to clone repository using all configured remotes.");
            return Err(eyre!("Failed to clone repository using all configured remotes."));
        }
    }

    SysCommand::new("git")
        .args(["checkout", &revision])
        .stdout(Stdio::null()) // Suppressing stdout output from the checkout command
        .status()
        .wrap_err("Failed to checkout the specified revision")?;

    Ok(())
}

fn fetch_revision_sha(remote_url: &str, repospec: &str, _verbose: bool) -> Result<String> {
    let separator = if remote_url.starts_with("git@") { ":" } else { "/" };
    let repo_url = format!("{}{}{}", remote_url, separator, repospec);

    let command_args = ["ls-remote", &repo_url, "HEAD"];
    debug!("Executing git command with args: {:?}", command_args);

    let output = SysCommand::new("git")
        .args(&command_args)
        .stdout(Stdio::null()) // Suppressing stdout output from the ls-remote command
        .output()
        .wrap_err("Failed to execute ls-remote")?;

    debug!("ls-remote output: {:?}", String::from_utf8_lossy(&output.stdout));

    let output_str = String::from_utf8(output.stdout)?;
    let sha = output_str.lines()
        .filter(|line| line.contains("HEAD"))
        .filter_map(|line| line.split_whitespace().next())
        .next()
        .ok_or_else(|| eyre!("Could not find SHA for HEAD"))
        .map(|s| s.to_string())?;

    Ok(sha)
}

fn attempt_clone(repospec: &str, full_clone_path: &Path, remote_url: &str, mirror_option: &Option<String>, _verbose: bool) -> Result<bool> {
    let mut clone_command = SysCommand::new("git");
    clone_command.arg("clone")
        .stdout(Stdio::null()) // Suppressing stdout output from the clone command
        .arg(format!("{}/{}", remote_url, repospec))
        .arg(full_clone_path);

    if let Some(ref mirror) = mirror_option {
        clone_command.arg(mirror);
    }

    debug!("Executing: {:?}", clone_command);

    let clone_status = clone_command.status()?;
    if !clone_status.success() {
        error!("Cloning failed for {}: {}", repospec, clone_status);
    }
    Ok(clone_status.success())
}
