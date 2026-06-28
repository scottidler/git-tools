// worktree — thin shell: parse args, init logging, delegate to the library.
//
// Modeled on `clone`: the binary prints the destination worktree path to stdout
// and the `worktree()` shell function `cd`s into it. Listing prints a table to
// stdout (the shell function passes the no-branch form straight through, so the
// table is shown rather than treated as a `cd` target).

use clap::Parser;
use eyre::Result;
use worktree::{Cli, Config, Outcome, run};

fn main() -> Result<()> {
    let cli = Cli::parse();
    common::log::init(cli.log_level, "worktree")?;

    let config = Config::try_from(cli)?;
    match run(config)? {
        Outcome::Switched(path) => println!("{}", path.display()),
        Outcome::Listed(entries) => print_entries(&entries),
        Outcome::Pruned(removed) => eprintln!("worktree: removed {} worktree(s)", removed.len()),
    }

    Ok(())
}

/// Print the worktree table to stdout, one branch per line, skipping the bare
/// container entry (it has no working tree to `cd` into).
fn print_entries(entries: &[worktree::Entry]) {
    for entry in entries {
        if entry.bare {
            continue;
        }
        let branch = entry.branch.as_deref().unwrap_or("(detached)");
        let lock = if entry.locked { " [locked]" } else { "" };
        println!("{:<28} {}{}", branch, entry.path.display(), lock);
    }
}
