// clone — validated configuration, built from `Cli` via `TryFrom`.

use std::env;
use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::warn;

use crate::cli::Cli;

/// Validated, resolved configuration consumed by [`crate::run`].
///
/// `Cli` is parsing only; `Config` carries the parsed `RepoSpec`, expanded
/// paths, and the per-org SSH key resolved from `clone.cfg`.
#[derive(Debug)]
pub struct Config {
    pub spec: RepoSpec,
    pub revision: String,
    pub remote: String,
    pub clonepath: PathBuf,
    pub mirrorpath: Option<PathBuf>,
    pub versioning: bool,
    pub verbose: bool,
    pub ssh_key: Option<PathBuf>,
}

impl TryFrom<Cli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: Cli) -> Result<Self> {
        let spec = git::parse_repospec(&cli.repospec)
            .wrap_err_with(|| format!("Failed to parse repository specification: {}", cli.repospec))?;
        let ssh_key = find_ssh_key_for_org(&spec.org)?.map(PathBuf::from);

        Ok(Self {
            spec,
            revision: cli.revision,
            remote: cli.remote,
            clonepath: PathBuf::from(cli.clonepath),
            mirrorpath: cli.mirrorpath.map(PathBuf::from),
            versioning: cli.versioning,
            verbose: cli.verbose,
            ssh_key,
        })
    }
}

/// Resolve the per-org transport SSH key from `clone.cfg`.
///
/// Reads `$CLONE_CFG` (or `~/.config/clone/clone.cfg`), looks up the
/// `[org.<org>]` section, falling back to `[org.default]`. Returns `Ok(None)`
/// when no config file is present. Accepts either an `org` or a full
/// `org/repo`; only the leading org component is used.
pub fn find_ssh_key_for_org(repospec: &str) -> Result<Option<String>> {
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
mod tests;
