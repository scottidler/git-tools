use clap::Parser;
use eyre::{Result, WrapErr};
use log::{LevelFilter, debug};

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "reposlug", about = "get the reposlug from the remote origin url")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
struct Args {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    log_level: LevelFilter,

    #[arg(short, long)]
    verbose: bool,

    #[arg(value_parser, help = "[default: .]")]
    directory: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    common::log::init(args.log_level, "reposlug")?;

    let directory = args.directory.unwrap_or_else(|| String::from("."));
    debug!("main: directory={directory} verbose={}", args.verbose);

    if args.verbose {
        println!("Using directory: {directory}");
    }

    let repo_slug = common::git::get_repo_slug_from_path(&directory).wrap_err("could not parse remote")?;
    debug!("main: resolved slug={repo_slug}");

    println!("{repo_slug}");

    Ok(())
}
