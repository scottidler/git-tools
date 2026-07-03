// clone — validated configuration, built from `Cli` via `TryFrom`.

use std::env;
use std::path::{Path, PathBuf};

use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::{debug, warn};

use crate::cli::Cli;

/// On-disk repository layout `clone` produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Bare container (`.bare/` + `.git` pointer + nested worktrees). Opt-in
    /// via `--bare` or `[clone] default-layout = bare` in `clone.cfg`.
    Bare,
    /// Single checkout, no worktrees. Default.
    Flat,
}

/// The operation `clone` performs this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Clone (or update) a repository. Requires `spec`.
    Clone,
    /// Convert an existing flat checkout into a bare container. `spec` is
    /// optional (derived from the current directory's enclosing repo when
    /// absent).
    Migrate,
}

/// Validated, resolved configuration consumed by [`crate::run`].
///
/// `Cli` is parsing only; `Config` carries the parsed `RepoSpec`, expanded
/// paths, the resolved layout + operation, and the per-org SSH key resolved
/// from `clone.cfg`.
#[derive(Debug)]
pub struct Config {
    /// `None` only for `--migrate` run inside a checkout (no `org/repo` arg).
    pub spec: Option<RepoSpec>,
    pub op: Op,
    pub layout: Layout,
    pub revision: String,
    pub remote: String,
    pub clonepath: PathBuf,
    pub mirrorpath: Option<PathBuf>,
    pub versioning: bool,
    pub verbose: bool,
    /// With `Op::Migrate`, preview the plan without changing anything.
    pub dry_run: bool,
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

        let op = if cli.migrate { Op::Migrate } else { Op::Clone };

        // Validation: only clone needs a repospec; --migrate can derive its
        // target from the current directory, so a repospec is optional there.
        if matches!(op, Op::Clone) && spec.is_none() {
            return Err(eyre!("a repository specification (org/repo or a URL) is required"));
        }

        // --flat (explicit, or implied by --versioning) selects the flat
        // layout, which has nothing to migrate to.
        if (cli.flat || cli.versioning) && matches!(op, Op::Migrate) {
            return Err(eyre!("--flat/--versioning cannot be combined with --migrate"));
        }

        // --bare and --flat/--versioning both name a layout; only one wins.
        if cli.bare && (cli.flat || cli.versioning) {
            return Err(eyre!("--bare cannot be combined with --flat/--versioning"));
        }

        // --migrate always produces a bare container; --bare is redundant there.
        if cli.bare && matches!(op, Op::Migrate) {
            return Err(eyre!("--bare cannot be combined with --migrate"));
        }

        // --dry-run only previews a migration; it is meaningless elsewhere.
        if cli.dry_run && !matches!(op, Op::Migrate) {
            return Err(eyre!("--dry-run is only valid with --migrate"));
        }

        let ssh_key = match &spec {
            Some(spec) => find_ssh_key_for_org(&spec.org)?.map(PathBuf::from),
            None => None,
        };
        let layout = resolve_layout(
            cli.bare,
            cli.flat,
            cli.versioning,
            clone_cfg_value("default-layout").as_deref(),
        );
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
            dry_run: cli.dry_run,
            ssh_key,
            default_branch,
        })
    }
}

/// Resolve the layout: CLI `--bare`/`--flat` (or `--versioning`, which implies
/// flat) > `clone.cfg` `[clone] default-layout` > built-in default (`Flat`).
///
/// `Config::try_from` already rejects `--bare` combined with `--flat`/
/// `--versioning`, so the flag checks below are mutually exclusive in
/// practice; the flat checks run first only as a defensive ordering for
/// direct callers (e.g. tests) that bypass that validation.
fn resolve_layout(bare_flag: bool, flat_flag: bool, versioning: bool, cfg_layout: Option<&str>) -> Layout {
    debug!(
        "resolve_layout: bare_flag={} flat_flag={} versioning={} cfg_layout={:?}",
        bare_flag, flat_flag, versioning, cfg_layout
    );
    if flat_flag || versioning {
        return Layout::Flat;
    }
    if bare_flag {
        return Layout::Bare;
    }
    match cfg_layout {
        Some(s) if s.eq_ignore_ascii_case("bare") => Layout::Bare,
        _ => Layout::Flat,
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
