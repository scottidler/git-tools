// clone — CLI parsing (clap derive only).

use clap::Parser;
use log::LevelFilter;

use crate::REMOTE_URLS;

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "Repository specification. Accepts: org/repo, https://github.com/org/repo, git@github.com:org/repo, ssh://git@github.com/org/repo, git://github.com/org/repo. Optional with --worktree run inside a container."
    )]
    pub repospec: Option<String>,

    #[arg(help = "revision to check out", default_value = "HEAD")]
    pub revision: String,

    #[arg(long, help = "the git URL to be used with git clone", default_value = REMOTE_URLS[0])]
    pub remote: String,

    #[arg(long, help = "path to store all cloned repos", default_value = ".")]
    pub clonepath: String,

    #[arg(long, help = "path to cached repos to support fast cloning")]
    pub mirrorpath: Option<String>,

    #[arg(
        long,
        help = "use the legacy flat single-checkout layout instead of bare + worktrees"
    )]
    pub flat: bool,

    #[arg(
        long,
        help = "add a worktree for <branch> to an existing bare container, then cd into it"
    )]
    pub worktree: Option<String>,

    #[arg(
        long,
        help = "convert a flat checkout into a bare container; with no repospec, migrates the checkout you're standing in"
    )]
    pub migrate: bool,

    #[arg(
        long,
        help = "with --migrate, print what would happen (worktrees, rescues, removals) without changing anything"
    )]
    pub dry_run: bool,

    #[arg(long, help = "turn on versioning; checkout in reponame/commit rather than reponame")]
    pub versioning: bool,

    #[arg(long, help = "turn on verbose output")]
    pub verbose: bool,
}
