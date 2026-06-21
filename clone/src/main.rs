// clone — thin shell: parse args, init logging, delegate to the library,
// print the destination path the wrapper `cd`s into.

use clap::Parser;
use clone::{Cli, Config, run};
use eyre::Result;

fn main() -> Result<()> {
    let cli = Cli::parse();
    common::log::init(cli.log_level, "clone")?;

    let config = Config::try_from(cli)?;
    let dest = run(config)?;

    println!("{}", dest.display());

    Ok(())
}
