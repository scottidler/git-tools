// worktree — core logic. The binary (`main.rs`) is a thin shell over `run`.
//
// Manages worktrees inside a bare container (the `.bare/` + `.git`-pointer +
// per-branch-worktree layout `clone` produces). Two operations:
//   * no branch        → list the container's worktrees;
//   * a branch argument → switch to (or create) that worktree, returning its
//     path for the shell wrapper to `cd` into.

pub mod bare;
pub mod cli;
pub mod config;
pub mod list;
pub mod pick;
pub mod prune;
pub mod shell;
pub mod switch;

pub use cli::Cli;
pub use config::{Config, Op};
pub use list::Entry;

use eyre::{Result, bail};
use log::debug;

/// What `run` produced this invocation.
#[derive(Debug)]
pub enum Outcome {
    /// The container's worktrees (for `main.rs` to format and print).
    Listed(Vec<Entry>),
    /// The worktree path the shell wrapper should `cd` into.
    Switched(std::path::PathBuf),
    /// The worktree paths `--prune` removed (user-facing reporting happens in
    /// `prune`; these are returned for the final count).
    Pruned(Vec<std::path::PathBuf>),
}

/// Resolve the enclosing bare container and perform the requested operation.
pub fn run(config: Config) -> Result<Outcome> {
    let container = bare::resolve_container_from_cwd()?;
    debug!("run: container={:?} op={:?}", container, config.op);

    if !bare::is_bare_container(&container) {
        bail!(
            "'{}' is not a bare container; worktree requires the bare layout (run `clone --migrate` first)",
            container.display()
        );
    }

    match config.op {
        Op::List => Ok(Outcome::Listed(list::list(&container)?)),
        Op::Pick => Ok(Outcome::Switched(pick::pick(&container)?)),
        Op::Prune => {
            let removed = prune::prune(&container, config.default_branch.as_deref(), config.assume_yes)?;
            Ok(Outcome::Pruned(removed))
        }
        Op::Switch(branch) => {
            let path = switch::switch(&container, &branch, config.default_branch.as_deref())?;
            Ok(Outcome::Switched(path))
        }
    }
}
