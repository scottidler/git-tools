// worktree — validated configuration, built from the CLI parsers via `TryFrom`.

use std::path::PathBuf;

use common::git::{self, RepoSpec};
use eyre::Result;

use crate::cli::{Cli, FlattenCli, InitCli, MigrateCli};

/// The operation `worktree` performs this invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Interactively pick a worktree (no branch, no `--list`/`--prune`).
    Pick,
    /// List the bare container's worktrees (`--list`).
    List,
    /// Remove merged worktrees (`--prune`).
    Prune,
    /// Switch to (or create) the worktree for the given raw branch argument.
    Switch(String),
    /// Fresh bare-container acquisition (`worktree init <spec>`).
    Init(RepoSpec),
    /// Convert a flat checkout into a bare container (`worktree migrate [spec]`).
    /// `None` derives the target from the current directory.
    Migrate(Option<RepoSpec>),
    /// Collapse a bare container into a flat checkout (`worktree flatten [spec]`).
    /// `None` derives the target from the current directory.
    Flatten(Option<RepoSpec>),
}

/// Validated, resolved configuration consumed by [`crate::run`].
///
/// The acquisition fields (`clonepath`/`remote`/`mirrorpath`/`verbose`/`dry_run`/
/// `ssh_key`) are only meaningful for `Init`/`Migrate`/`Flatten`; the local ops
/// (`Pick`/`List`/`Prune`/`Switch`) leave them at benign defaults.
#[derive(Debug)]
pub struct Config {
    pub op: Op,
    /// Last-resort base/default branch, used only when the remote default branch
    /// can't be detected from the container/remote.
    pub default_branch: Option<String>,
    /// `--yes`: skip the `--prune` confirmation prompt.
    pub assume_yes: bool,
    /// Root a bare container is created/addressed under (`<clonepath>/<org>/<repo>`).
    pub clonepath: PathBuf,
    /// Primary (SSH) remote base for `init`; HTTPS is the fallback.
    pub remote: String,
    /// Optional reference mirror for a fast `init` clone.
    pub mirrorpath: Option<PathBuf>,
    /// Verbose transport logging (`init`).
    pub verbose: bool,
    /// `--dry-run`: preview a `migrate`/`flatten` conversion without changing anything.
    pub dry_run: bool,
    /// Per-org SSH key for the `init` clone, resolved from the shared config.
    pub ssh_key: Option<PathBuf>,
}

impl Config {
    /// Defaults for the local day-2 ops, which carry no acquisition inputs.
    fn local(op: Op, default_branch: Option<String>, assume_yes: bool) -> Self {
        Self {
            op,
            default_branch,
            assume_yes,
            clonepath: PathBuf::from("."),
            remote: common::transport::REMOTE_URLS[0].to_string(),
            mirrorpath: None,
            verbose: false,
            dry_run: false,
            ssh_key: None,
        }
    }
}

impl TryFrom<Cli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: Cli) -> Result<Self> {
        if cli.list && cli.prune {
            return Err(eyre::eyre!("--list and --prune are mutually exclusive"));
        }
        if (cli.list || cli.prune) && cli.branch.is_some() {
            return Err(eyre::eyre!("--list/--prune take no branch argument"));
        }
        let op = match (cli.prune, cli.list, cli.branch) {
            (true, _, _) => Op::Prune,
            (_, true, _) => Op::List,
            (_, _, Some(branch)) => Op::Switch(branch),
            (_, _, None) => Op::Pick,
        };
        Ok(Self::local(op, cli.default_branch, cli.yes))
    }
}

impl TryFrom<InitCli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: InitCli) -> Result<Self> {
        let spec = git::parse_repospec(&cli.spec)
            .map_err(|e| eyre::eyre!("Failed to parse repository specification '{}': {}", cli.spec, e))?;
        let ssh_key = common::config::find_ssh_key_for_org(&spec.org)?.map(PathBuf::from);
        let default_branch = common::config::clone_cfg_value("default");
        Ok(Self {
            op: Op::Init(spec),
            default_branch,
            assume_yes: false,
            clonepath: PathBuf::from(cli.clonepath),
            remote: cli.remote,
            mirrorpath: cli.mirrorpath.map(PathBuf::from),
            verbose: cli.verbose,
            dry_run: false,
            ssh_key,
        })
    }
}

impl TryFrom<MigrateCli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: MigrateCli) -> Result<Self> {
        let spec = parse_optional_spec(cli.spec.as_deref())?;
        let default_branch = common::config::clone_cfg_value("default");
        Ok(Self {
            op: Op::Migrate(spec),
            default_branch,
            assume_yes: false,
            clonepath: PathBuf::from(cli.clonepath),
            remote: common::transport::REMOTE_URLS[0].to_string(),
            mirrorpath: None,
            verbose: false,
            dry_run: cli.dry_run,
            ssh_key: None,
        })
    }
}

impl TryFrom<FlattenCli> for Config {
    type Error = eyre::Report;

    fn try_from(cli: FlattenCli) -> Result<Self> {
        let spec = parse_optional_spec(cli.spec.as_deref())?;
        let default_branch = common::config::clone_cfg_value("default");
        Ok(Self {
            op: Op::Flatten(spec),
            default_branch,
            assume_yes: false,
            clonepath: PathBuf::from(cli.clonepath),
            remote: common::transport::REMOTE_URLS[0].to_string(),
            mirrorpath: None,
            verbose: false,
            dry_run: cli.dry_run,
            ssh_key: None,
        })
    }
}

/// Parse an optional `[spec]` positional (migrate/flatten derive their target
/// from cwd when absent).
fn parse_optional_spec(spec: Option<&str>) -> Result<Option<RepoSpec>> {
    match spec {
        Some(s) => Ok(Some(git::parse_repospec(s).map_err(|e| {
            eyre::eyre!("Failed to parse repository specification '{}': {}", s, e)
        })?)),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests;
