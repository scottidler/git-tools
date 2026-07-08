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
pub mod flatten;
pub mod init;
pub mod list;
pub mod migrate;
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
    /// A `migrate`/`flatten` `--dry-run` preview. The human-readable plan was
    /// already written to stderr by `migrate::dry_run`/`flatten::dry_run`; the
    /// path is carried for callers/tests that want it, but `main.rs` prints
    /// NOTHING for this variant so stdout stays empty and the `worktree()`
    /// wrapper's empty-output guard (`shell.rs`) short-circuits before any `cd`.
    Previewed(std::path::PathBuf),
}

/// Perform the requested operation.
///
/// The acquisition/conversion verbs (`init`, and explicit-spec `migrate`/
/// `flatten`) run from ANY cwd: they address their target by spec, so they do
/// NOT resolve an enclosing container. Only the local day-2 ops (`switch`/`list`/
/// `prune`/`pick`) and the no-spec `migrate`/`flatten` forms resolve the bare
/// container from the current directory. The cwd resolution therefore lives
/// inside the local-op arm, not at the top of `run`.
pub fn run(config: Config) -> Result<Outcome> {
    debug!("run: op={:?}", config.op);
    match &config.op {
        Op::Init(spec) => Ok(Outcome::Switched(init::init(&config, spec)?)),
        Op::Migrate(spec) => {
            // With a spec, the target is `<clonepath>/<org>/<repo>`; with no spec,
            // migrate the flat checkout the user is standing in.
            let flat = match spec {
                Some(spec) => config.clonepath.join(spec.to_string()),
                None => migrate::flat_from_cwd()?,
            };
            if config.dry_run {
                let path = migrate::dry_run(&flat, config.default_branch.as_deref())?;
                return Ok(Outcome::Previewed(path));
            }
            let path = migrate::migrate_flat_to_bare(&flat, config.default_branch.as_deref())?;
            Ok(Outcome::Switched(path))
        }
        Op::Flatten(spec) => {
            // With a spec, the target is `<clonepath>/<org>/<repo>`; with no spec,
            // flatten the container the user is standing in.
            let container = match spec {
                Some(spec) => config.clonepath.join(spec.to_string()),
                None => flatten::container_from_cwd()?,
            };
            if config.dry_run {
                let path = flatten::dry_run(&container, config.default_branch.as_deref())?;
                return Ok(Outcome::Previewed(path));
            }
            let path = flatten::flatten(&container, config.default_branch.as_deref())?;
            Ok(Outcome::Switched(path))
        }
        _ => run_local(config),
    }
}

/// The day-2 worktree ops that require an enclosing bare container resolved from
/// the current directory (`switch`/`list`/`prune`/`pick`).
fn run_local(config: Config) -> Result<Outcome> {
    let container = bare::resolve_container_from_cwd()?;
    debug!("run_local: container={:?} op={:?}", container, config.op);

    if !bare::is_bare_container(&container) {
        bail!(
            "'{}' is not a bare container; worktree requires the bare layout (run `worktree migrate` first)",
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
        // Init/Migrate/Flatten are dispatched in `run` before reaching here.
        Op::Init(_) | Op::Migrate(_) | Op::Flatten(_) => {
            unreachable!("acquisition verbs are dispatched in run(), never run_local()")
        }
    }
}
