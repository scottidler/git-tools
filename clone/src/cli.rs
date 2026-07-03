// clone — CLI parsing (clap derive only).

use clap::Parser;
use log::LevelFilter;

use crate::REMOTE_URLS;

#[derive(Parser, Debug)]
#[command(name = "clone", about = "Clones repositories with optional versioning and mirroring")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
#[command(
    after_help = "Layout: `clone <org>/<repo>` produces a flat checkout by default; pass --bare for a bare container + worktrees (or set `[clone] default-layout = bare` in clone.cfg). Convert between layouts with `clone --migrate` (flat -> bare) and `clone --flatten` (bare -> flat; refuses on any unmergeable/unsafe worktree state). Shell integration: `clone shell-init zsh` prints a cd-wrapper function; install it with `eval \"$(command clone shell-init zsh)\"` in your .zshrc so clone cd's you into the new checkout."
)]
pub struct Cli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "Repository specification. Accepts: org/repo, https://github.com/org/repo, git@github.com:org/repo, ssh://git@github.com/org/repo, git://github.com/org/repo. Optional with --migrate run inside a checkout."
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

    #[arg(long, help = "use a bare container + worktrees layout instead of a flat checkout")]
    pub bare: bool,

    #[arg(
        long,
        help = "use the flat single-checkout layout (default; retained as a no-op alias)"
    )]
    pub flat: bool,

    #[arg(
        long,
        help = "convert a flat checkout into a bare container; with no repospec, migrates the checkout you're standing in"
    )]
    pub migrate: bool,

    #[arg(
        long,
        help = "collapse a bare container back into a flat checkout; refuses on any unsafe/unmergeable worktree state. With no repospec, flattens the container you're standing in"
    )]
    pub flatten: bool,

    #[arg(
        long,
        help = "with --migrate or --flatten, print what would happen (worktrees, refs, removals) without changing anything"
    )]
    pub dry_run: bool,

    #[arg(long, help = "turn on versioning; checkout in reponame/commit rather than reponame")]
    pub versioning: bool,

    #[arg(long, help = "turn on verbose output")]
    pub verbose: bool,
}
