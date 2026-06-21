// clone — validated configuration, built from `Cli` via `TryFrom`.

use std::env;
use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::warn;

use crate::cli::Cli;

/// On-disk repository layout `clone` produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Bare container (`.bare/` + `.git` pointer + nested worktrees). Default.
    Bare,
    /// Legacy single checkout (the pre-worktree behavior).
    Flat,
}

/// The operation `clone` performs this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Clone (or update) a repository. Requires `spec`.
    Clone,
    /// Add a worktree for the given (raw) branch argument to an existing bare
    /// container. `spec` is optional (derived from CWD when absent).
    AddWorktree(String),
    /// Convert an existing flat checkout into a bare container. Requires `spec`.
    Migrate,
}

/// Validated, resolved configuration consumed by [`crate::run`].
///
/// `Cli` is parsing only; `Config` carries the parsed `RepoSpec`, expanded
/// paths, the resolved layout + operation, and the per-org SSH key resolved
/// from `clone.cfg`.
#[derive(Debug)]
pub struct Config {
    /// `None` only for `--worktree` run inside a container (no `org/repo` arg).
    pub spec: Option<RepoSpec>,
    pub op: Op,
    pub layout: Layout,
    pub revision: String,
    pub remote: String,
    pub clonepath: PathBuf,
    pub mirrorpath: Option<PathBuf>,
    pub versioning: bool,
    pub verbose: bool,
    pub ssh_key: Option<PathBuf>,
    /// Last-resort default branch from `clone.cfg` `[clone] default`, used only
    /// when the remote does not advertise a default branch.
    pub default_branch: Option<String>,
}

impl TryFrom<Cli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: Cli) -> Result<Self> {
        let spec = match &cli.repospec {
            Some(s) => Some(
                git::parse_repospec(s).wrap_err_with(|| format!("Failed to parse repository specification: {}", s))?,
            ),
            None => None,
        };

        if cli.worktree.is_some() && cli.migrate {
            return Err(eyre!("--worktree cannot be combined with --migrate"));
        }

        let op = match (cli.worktree, cli.migrate) {
            (Some(branch), _) => Op::AddWorktree(branch),
            (None, true) => Op::Migrate,
            (None, false) => Op::Clone,
        };

        // Validation: clone and migrate need a repospec; --worktree can derive
        // its container from CWD, so a repospec is optional there.
        if matches!(op, Op::Clone | Op::Migrate) && spec.is_none() {
            return Err(eyre!("a repository specification (org/repo or a URL) is required"));
        }

        // --flat selects the legacy layout, which has no worktrees and nothing
        // to migrate to.
        if cli.flat && matches!(op, Op::AddWorktree(_) | Op::Migrate) {
            return Err(eyre!("--flat cannot be combined with --worktree or --migrate"));
        }

        let ssh_key = match &spec {
            Some(spec) => find_ssh_key_for_org(&spec.org)?.map(PathBuf::from),
            None => None,
        };
        let layout = resolve_layout(cli.flat, cli.versioning, clone_cfg_value("default-layout").as_deref());
        let default_branch = clone_cfg_value("default");

        Ok(Self {
            spec,
            op,
            layout,
            revision: cli.revision,
            remote: cli.remote,
            clonepath: PathBuf::from(cli.clonepath),
            mirrorpath: cli.mirrorpath.map(PathBuf::from),
            versioning: cli.versioning,
            verbose: cli.verbose,
            ssh_key,
            default_branch,
        })
    }
}

/// Resolve the layout: CLI `--flat` (or `--versioning`, which is incompatible
/// with bare worktrees) > `clone.cfg` `[clone] default-layout` > `Bare`.
fn resolve_layout(flat_flag: bool, versioning: bool, cfg_layout: Option<&str>) -> Layout {
    if flat_flag || versioning {
        return Layout::Flat;
    }
    match cfg_layout {
        Some(s) if s.eq_ignore_ascii_case("flat") => Layout::Flat,
        _ => Layout::Bare,
    }
}

/// Read a single value from the `[clone]` section of `clone.cfg`, if the file
/// and key are present. Honors `$CLONE_CFG`, else `~/.config/clone/clone.cfg`.
fn clone_cfg_value(key: &str) -> Option<String> {
    let home = env::var("HOME").ok()?;
    let path = env::var("CLONE_CFG").unwrap_or_else(|_| format!("{}/.config/clone/clone.cfg", home));
    if !Path::new(&path).exists() {
        return None;
    }
    let cfg = ini!(&path);
    cfg.get("clone").and_then(|m| m.get(key).cloned().flatten())
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
