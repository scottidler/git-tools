// clone — validated configuration, built from `Cli` via `TryFrom`.

use std::path::PathBuf;

use common::config::find_ssh_key_for_org;
use common::git::{self, RepoSpec};
use eyre::{Result, WrapErr, eyre};

use crate::cli::Cli;

/// The operation `clone` performs this invocation. Always a flat checkout;
/// bare-container acquisition and layout conversion live on `worktree`
/// (`init`/`migrate`/`flatten`), not here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// Clone (or update) a repository. Requires `spec`.
    Clone,
}

/// Validated, resolved configuration consumed by [`crate::run`].
///
/// `Cli` is parsing only; `Config` carries the parsed `RepoSpec`, expanded
/// paths, the operation, and the per-org SSH key resolved from `clone.cfg`.
#[derive(Debug)]
pub struct Config {
    pub spec: Option<RepoSpec>,
    pub op: Op,
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
        let spec = match &cli.repospec {
            Some(s) => Some(
                git::parse_repospec(s).wrap_err_with(|| format!("Failed to parse repository specification: {}", s))?,
            ),
            None => None,
        };

        // clone always needs a repospec now: the no-repospec forms (bare-cwd
        // `--migrate`/`--flatten`) moved to `worktree migrate`/`worktree flatten`.
        if spec.is_none() {
            return Err(eyre!("a repository specification (org/repo or a URL) is required"));
        }

        let ssh_key = match &spec {
            Some(spec) => find_ssh_key_for_org(&spec.org)?.map(PathBuf::from),
            None => None,
        };

        Ok(Self {
            spec,
            op: Op::Clone,
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

#[cfg(test)]
mod tests;
