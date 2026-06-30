// clone — thin shell: parse args, init logging, delegate to the library,
// print the destination path the wrapper `cd`s into.

use clap::Parser;
use clone::{Cli, Config, run, shell};
use eyre::Result;

fn main() -> Result<()> {
    // Pre-dispatch: if the first argument is exactly "shell-init", emit the
    // requested shell's wrapper script and return before clap ever sees the
    // token.  This keeps the bare-positional `clone <repospec>` interface
    // byte-for-byte unchanged.
    let mut raw = std::env::args().skip(1);
    if raw.next().as_deref() == Some("shell-init") {
        let target = raw.next().unwrap_or_else(|| "zsh".to_string());
        print!("{}", shell::init_script(&target)?);
        return Ok(());
    }

    let cli = Cli::parse();
    common::log::init(cli.log_level, "clone")?;

    let config = Config::try_from(cli)?;
    let dest = run(config)?;

    println!("{}", dest.display());

    Ok(())
}
