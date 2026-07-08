// worktree — thin shell: parse args, init logging, delegate to the library.
//
// Modeled on `clone`: the binary prints the destination worktree path to stdout
// and the `worktree()` shell function `cd`s into it. Listing prints a table to
// stdout (the shell function passes the no-branch form straight through, so the
// table is shown rather than treated as a `cd` target).
//
// Pre-clap dispatch: when `argv[1]` is a reserved verb (`shell-init`, `init`,
// `migrate`, `flatten`) it is intercepted before `Cli::parse` so clap never
// mistakes it for the positional `branch`. Each acquisition verb is handed to its
// own clap parser via `parse_from` (the verb token fills the binary-name slot,
// with `#[command(name = "worktree <verb>")]` keeping usage honest). Contract:
// the verb (or branch) is `argv[1]`; leading global flags before it are
// unsupported, exactly as the branch form already is.

use clap::Parser;
use eyre::Result;
use worktree::cli::{FlattenCli, InitCli, MigrateCli};
use worktree::{Cli, Config, Outcome, run, shell};

fn main() -> Result<()> {
    let verb = std::env::args().nth(1);
    match verb.as_deref() {
        Some("shell-init") => {
            let target = std::env::args().nth(2).unwrap_or_else(|| "zsh".to_string());
            print!("{}", shell::init_script(&target)?);
            return Ok(());
        }
        // Each verb consumes `argv[1]` as its bin-name slot; the flags/spec that
        // follow (`argv[2..]`) are what its parser sees.
        Some("init") => {
            let cli = InitCli::parse_from(std::env::args().skip(1));
            common::log::init(cli.log_level, "worktree")?;
            return dispatch(Config::try_from(cli)?);
        }
        Some("migrate") => {
            let cli = MigrateCli::parse_from(std::env::args().skip(1));
            common::log::init(cli.log_level, "worktree")?;
            return dispatch(Config::try_from(cli)?);
        }
        Some("flatten") => {
            let cli = FlattenCli::parse_from(std::env::args().skip(1));
            common::log::init(cli.log_level, "worktree")?;
            return dispatch(Config::try_from(cli)?);
        }
        _ => {}
    }

    let cli = Cli::parse();
    common::log::init(cli.log_level, "worktree")?;
    dispatch(Config::try_from(cli)?)
}

/// Run the resolved config and emit its outcome (path to stdout for `cd`, table
/// to stdout for a list, count to stderr for a prune). A `--dry-run` preview
/// prints NOTHING to stdout: `migrate::dry_run`/`flatten::dry_run` already wrote
/// the human-readable plan to stderr, and stdout must stay empty so the
/// `worktree()` wrapper's empty-output guard bails before any `cd`.
fn dispatch(config: Config) -> Result<()> {
    match run(config)? {
        Outcome::Switched(path) => println!("{}", path.display()),
        Outcome::Listed(entries) => print_entries(&entries),
        Outcome::Pruned(removed) => eprintln!("worktree: removed {} worktree(s)", removed.len()),
        Outcome::Previewed(_path) => {}
    }
    Ok(())
}

/// Print the worktree table to stdout, one branch per line, skipping the bare
/// container entry (it has no working tree to `cd` into). The branch column is
/// padded to the widest branch name so the path column stays aligned even when a
/// branch name is longer than any fixed width would allow.
fn print_entries(entries: &[worktree::Entry]) {
    let label = |entry: &worktree::Entry| entry.branch.clone().unwrap_or_else(|| "(detached)".into());

    let width = entries
        .iter()
        .filter(|entry| !entry.bare)
        .map(|entry| label(entry).chars().count())
        .max()
        .unwrap_or(0);

    for entry in entries {
        if entry.bare {
            continue;
        }
        let branch = label(entry);
        let lock = if entry.locked { " [locked]" } else { "" };
        println!("{:<width$} {}{}", branch, entry.path.display(), lock, width = width);
    }
}
