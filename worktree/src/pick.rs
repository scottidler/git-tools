// worktree — interactive fzf picker for the no-arg form.
//
// `worktree` with no branch presents an fzf chooser over the container's
// worktrees and returns the selected path for the shell wrapper to `cd` into.
// The contract that makes this composable with `dest=$(worktree)`:
//   * fzf draws its UI on /dev/tty (its default), NOT on our stdout — so the
//     only thing on our stdout is the chosen path;
//   * interactivity is detected on STDIN, never stdout (stdout is a pipe under
//     command substitution, so `stdout.is_terminal()` is always false there);
//   * if fzf is absent or there is no tty we fail explicitly pointing at
//     `--list`, rather than silently degrading to a table (a table on stdout
//     would be swallowed by the wrapper's `[[ -d "$dest" ]]` check).

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use eyre::{Result, WrapErr, bail};
use log::debug;

use crate::list;

/// Present an fzf picker over `container`'s worktrees, returning the chosen
/// worktree path. Errors (non-zero) on cancellation so the wrapper stays put
/// quietly.
pub fn pick(container: &Path) -> Result<PathBuf> {
    debug!("pick: container={:?}", container);

    if !std::io::stdin().is_terminal() {
        bail!("worktree: no branch given and not a terminal; pass a branch or use `worktree --list`");
    }
    if !fzf_present() {
        bail!("worktree: fzf not found; pass a branch or use `worktree --list`");
    }

    // Selectable worktrees: skip the bare entry (no working tree to cd into).
    let entries: Vec<list::Entry> = list::list(container)?.into_iter().filter(|e| !e.bare).collect();
    if entries.is_empty() {
        bail!("worktree: no worktrees to pick from in '{}'", container.display());
    }

    // Feed fzf "<label>\t<path>"; show only the label column, return the path.
    let input: String = entries
        .iter()
        .map(|e| {
            let branch = e.branch.as_deref().unwrap_or("(detached)");
            let lock = if e.locked { " [locked]" } else { "" };
            format!("{}{}\t{}", branch, lock, e.path.display())
        })
        .collect::<Vec<_>>()
        .join("\n");

    let path = run_fzf(&input)?;
    Ok(PathBuf::from(path))
}

/// Whether `fzf` is on PATH.
fn fzf_present() -> bool {
    Command::new("fzf")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run fzf over the tab-delimited `input`, returning the path field of the
/// selected line. fzf draws on /dev/tty; we capture only its stdout.
fn run_fzf(input: &str) -> Result<String> {
    let mut child = Command::new("fzf")
        .args(["--delimiter", "\t", "--with-nth", "1", "--prompt", "worktree> "])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .wrap_err("failed to launch fzf")?;

    // Worktree lists are tiny; write all input then close stdin before reading.
    child
        .stdin
        .take()
        .expect("fzf stdin was piped")
        .write_all(input.as_bytes())
        .wrap_err("writing worktree list to fzf")?;

    let out = child.wait_with_output().wrap_err("waiting on fzf")?;
    if !out.status.success() {
        // Cancellation (Esc / Ctrl-C, exit 130) lands here: propagate non-zero so
        // the shell wrapper returns quietly without a "no destination" message.
        bail!("worktree: selection cancelled");
    }

    parse_selection(&String::from_utf8_lossy(&out.stdout))
}

/// Pull the path field out of a selected `"<label>\t<path>"` line.
fn parse_selection(selection: &str) -> Result<String> {
    let line = selection.trim();
    line.split('\t')
        .nth(1)
        .map(str::to_string)
        .ok_or_else(|| eyre::eyre!("fzf returned an unexpected line: {:?}", line))
}

#[cfg(test)]
mod tests;
