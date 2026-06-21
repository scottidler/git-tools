use clap::Parser;
use common::language::{detect_language, matches_language};
use common::repo::RepoDiscovery;
use eyre::{Result, eyre};
use log::{LevelFilter, debug};
use rayon::prelude::*;
use std::path::PathBuf;

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[command(
    name = "ls-git-repos",
    about = "List all local Git repositories with their GitHub reposlug"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = false)]
struct Cli {
    // `-l` is taken by `--lang`, so log-level is long-only here.
    #[arg(long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    log_level: LevelFilter,

    #[clap(value_parser, default_value = ".")]
    path: String,

    #[clap(short, long, num_args = 1..)]
    lang: Vec<String>,
}

fn main() -> Result<()> {
    let args = Cli::parse();
    common::log::init(args.log_level, "ls-git-repos")?;

    let expanded_path = shellexpand::tilde(&args.path).to_string();
    let base_path = PathBuf::from(&expanded_path);

    if !base_path.exists() {
        return Err(eyre!("The specified path does not exist: {}", base_path.display()));
    }
    debug!("main: path={} lang={:?}", base_path.display(), args.lang);

    // Unbounded depth: WalkDir was infinite-depth, so RepoDiscovery's default
    // 2-level cap would silently stop finding deeply-nested repos.
    let discovery = RepoDiscovery::new(vec![expanded_path]).with_max_depth(None);
    let repos = discovery.discover()?;
    debug!("main: discovered {} repos", repos.len());

    let mut results: Vec<String> = if args.lang.is_empty() {
        repos.into_iter().map(|repo| repo.slug).collect()
    } else {
        repos
            .par_iter()
            .filter_map(|repo| {
                let detected = detect_language(&repo.path);
                if matches_language(detected.as_deref(), &args.lang) {
                    Some(repo.slug.clone())
                } else {
                    None
                }
            })
            .collect()
    };

    results.sort();
    for slug in results {
        println!("{slug}");
    }

    Ok(())
}
