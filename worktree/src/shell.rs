// worktree::shell - emit the zsh cd-wrapper function via `worktree shell-init zsh`.
//
// The GIT_DESCRIBE version marker in the script header is produced by build.rs,
// which already sets `GIT_DESCRIBE` for the `--version` flag in cli.rs; we
// reuse the same env variable so there is only one source of truth.

use log::debug;

/// Shells this crate actually supports.  The supported list lives here, next
/// to the function bodies, so the error message can never claim a shell this
/// binary does not emit.
const SUPPORTED: &[&str] = &["zsh"];

/// Emitted `worktree()` zsh function.
///
/// Header comment carries the binary version (GIT_DESCRIBE) so a stale
/// function in a long-running shell is diagnosable by comparing the comment
/// to `worktree --version`.
///
/// Behaviours baked in:
/// - `command worktree` bypasses the same-named function (no `$WORKTREE` env var).
/// - `shell-init` joins the `-*` passthrough case so an interactive
///   `worktree shell-init zsh` after the function is loaded prints the script
///   instead of trying to `cd` into it.
/// - Destination validated (`-z "$dest" || ! -d "$dest"`) before `cd`.
/// - Flags (`-*`) pass straight through so `--list`, `--prune`, etc. reach the
///   binary without triggering the capture branch.
const ZSH: &str = concat!(
    "# worktree - switch/create git worktrees in a bare container [shell-init ",
    env!("GIT_DESCRIBE"),
    "]\n",
    "# Install: add to your .zshrc -> if hash worktree 2>/dev/null; then eval \"$(command worktree shell-init zsh)\"; fi\n",
    "worktree() {\n",
    "    case \"$1\" in\n",
    "        -*|shell-init)\n",
    "            command worktree \"$@\"\n",
    "            ;;\n",
    "        *)\n",
    "            local dest\n",
    "            dest=$(command worktree \"$@\") || return $?\n",
    "            if [[ -z \"$dest\" || ! -d \"$dest\" ]]; then\n",
    "                print -u2 -- \"worktree: no valid destination returned; staying in $PWD\"\n",
    "                return 1\n",
    "            fi\n",
    "            cd \"$dest\"\n",
    "            ;;\n",
    "    esac\n",
    "}\n"
);

/// Return the shell-init script for `shell`, or an error naming the supported
/// shells.
pub fn init_script(shell: &str) -> eyre::Result<String> {
    debug!("init_script: shell={}", shell);
    match shell {
        "zsh" => {
            debug!("init_script: returning zsh script ({} bytes)", ZSH.len());
            Ok(ZSH.to_string())
        }
        other => Err(common::shell::unsupported("worktree", SUPPORTED, other)),
    }
}

#[cfg(test)]
mod tests;
