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
        help = "Repository specification. Accepts: org/repo, https://github.com/org/repo, git@github.com:org/repo, ssh://git@github.com/org/repo, git://github.com/org/repo",
        required = true
    )]
    pub repospec: String,

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

    #[arg(long, help = "turn on versioning; checkout in reponame/commit rather than reponame")]
    pub versioning: bool,

    #[arg(long, help = "turn on verbose output")]
    pub verbose: bool,
}
