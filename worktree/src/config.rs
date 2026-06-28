// worktree — validated configuration, built from `Cli` via `TryFrom`.

use eyre::Result;

use crate::cli::Cli;

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
}

/// Validated, resolved configuration consumed by [`crate::run`].
#[derive(Debug)]
pub struct Config {
    pub op: Op,
    /// Last-resort base branch for a NEW worktree, used only when the remote
    /// default branch can't be detected from the container.
    pub default_branch: Option<String>,
    /// `--yes`: skip the `--prune` confirmation prompt.
    pub assume_yes: bool,
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
        Ok(Self {
            op,
            default_branch: cli.default_branch,
            assume_yes: cli.yes,
        })
    }
}

#[cfg(test)]
mod tests;
