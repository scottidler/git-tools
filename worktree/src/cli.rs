// worktree — CLI parsing (clap derive only).
//
// The bare-positional `worktree <branch>` interface is parsed by `Cli`. The
// acquisition verbs (`init`/`migrate`/`flatten`) each have their own parser
// (`InitCli`/`MigrateCli`/`FlattenCli`), dispatched pre-clap in `main.rs` when
// the verb is `argv[1]` so clap never mistakes the verb for the positional
// branch. The verb token fills the binary-name slot of `parse_from`; `#[command(
// name = "worktree <verb>")]` keeps `--help` usage honest.

use clap::Parser;
use common::transport::REMOTE_URLS;
use log::LevelFilter;

#[derive(Parser, Debug)]
#[command(
    name = "worktree",
    about = "Switch between or create git worktrees in a bare container"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(
    after_help = "Shell integration: `worktree shell-init zsh` prints a cd-wrapper function; install it with `eval \"$(command worktree shell-init zsh)\"` in your .zshrc so worktree cd's you into the selected worktree."
)]
pub struct Cli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "branch to switch to or create a worktree for (existing local/remote branch matched as-is; a new branch is slugified). Omit for an interactive fzf picker."
    )]
    pub branch: Option<String>,

    #[arg(
        short = 'L',
        long,
        help = "list the container's worktrees instead of switching (no cd)"
    )]
    pub list: bool,

    #[arg(
        long,
        help = "remove worktrees whose branch is merged into origin/<default> (branch refs kept; reflects the last fetch)"
    )]
    pub prune: bool,

    #[arg(
        short = 'y',
        long,
        help = "skip the confirmation prompt for --prune (required when non-interactive)"
    )]
    pub yes: bool,

    #[arg(
        long,
        help = "branch to base a NEW worktree on when the remote default can't be detected"
    )]
    pub default_branch: Option<String>,
}

/// `worktree init <spec>` — fresh bare-container acquisition. Mirrors clone's
/// bare-acquisition inputs (NOT `--versioning`, a flat-only feature that stays on
/// `clone`); the per-org SSH key and default-branch fallback are derived from the
/// shared config, not flags.
#[derive(Parser, Debug)]
#[command(
    name = "worktree init",
    about = "Create a fresh bare container and cd into its default worktree"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
pub struct InitCli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "Repository specification. Accepts: org/repo, https://github.com/org/repo, git@github.com:org/repo, ssh://git@github.com/org/repo, git://github.com/org/repo."
    )]
    pub spec: String,

    #[arg(long, help = "the git URL to be used with git clone", default_value = REMOTE_URLS[0])]
    pub remote: String,

    #[arg(
        long,
        help = "path the bare container is created under (<clonepath>/<org>/<repo>)",
        default_value = "."
    )]
    pub clonepath: String,

    #[arg(long, help = "path to cached repos to support fast cloning")]
    pub mirrorpath: Option<String>,

    #[arg(long, help = "turn on verbose output")]
    pub verbose: bool,
}

/// `worktree migrate [spec]` — convert a flat checkout into a bare container.
/// With a spec, targets `<clonepath>/<org>/<repo>`; with no spec, migrates the
/// checkout the user is standing in.
#[derive(Parser, Debug)]
#[command(
    name = "worktree migrate",
    about = "Convert a flat checkout into a bare container (flat -> bare)"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
pub struct MigrateCli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "Repository specification (org/repo or a URL). Optional: with no spec, migrates the checkout you're standing in."
    )]
    pub spec: Option<String>,

    #[arg(
        long,
        help = "path the target container lives under (<clonepath>/<org>/<repo>)",
        default_value = "."
    )]
    pub clonepath: String,

    #[arg(
        long,
        help = "print what would happen (worktrees, refs, removals) without changing anything"
    )]
    pub dry_run: bool,
}

/// `worktree flatten [spec]` — collapse a bare container back into a flat
/// checkout. Refuses on any unsafe/unmergeable worktree state. With no spec,
/// flattens the container the user is standing in.
#[derive(Parser, Debug)]
#[command(
    name = "worktree flatten",
    about = "Collapse a bare container back into a flat checkout (bare -> flat)"
)]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
pub struct FlattenCli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    pub log_level: LevelFilter,

    #[arg(
        help = "Repository specification (org/repo or a URL). Optional: with no spec, flattens the container you're standing in."
    )]
    pub spec: Option<String>,

    #[arg(
        long,
        help = "path the target container lives under (<clonepath>/<org>/<repo>)",
        default_value = "."
    )]
    pub clonepath: String,

    #[arg(
        long,
        help = "print what would happen (retained refs, refusals, ignored files) without changing anything"
    )]
    pub dry_run: bool,
}
