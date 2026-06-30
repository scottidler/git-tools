// worktree — CLI parsing (clap derive only).

use clap::Parser;
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
