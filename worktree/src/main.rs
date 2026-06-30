// worktree — thin shell: parse args, init logging, delegate to the library.
//
// Modeled on `clone`: the binary prints the destination worktree path to stdout
// and the `worktree()` shell function `cd`s into it. Listing prints a table to
// stdout (the shell function passes the no-branch form straight through, so the
// table is shown rather than treated as a `cd` target).

use clap::Parser;
use eyre::Result;
use worktree::{Cli, Config, Outcome, run, shell};

fn main() -> Result<()> {
    // Pre-dispatch: if the first argument is exactly "shell-init", emit the
    // requested shell's wrapper script and return before clap ever sees the
    // token.  This keeps the bare-positional `worktree <branch>` interface
    // byte-for-byte unchanged.
    let mut raw = std::env::args().skip(1);
    if raw.next().as_deref() == Some("shell-init") {
        let target = raw.next().unwrap_or_else(|| "zsh".to_string());
        print!("{}", shell::init_script(&target)?);
        return Ok(());
    }

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
