// clone::shell - emit the zsh cd-wrapper function via `clone shell-init zsh`.
//
// The GIT_DESCRIBE version marker in the script header is produced by build.rs,
// which already sets `GIT_DESCRIBE` for the `--version` flag in cli.rs; we
// reuse the same env variable so there is only one source of truth.

use log::debug;

/// Shells this crate actually supports.  The supported list lives here, next
/// to the function bodies, so the error message can never claim a shell this
/// binary does not emit.
const SUPPORTED: &[&str] = &["zsh"];

/// Emitted `clone()` zsh function.
///
/// Header comment carries the binary version (GIT_DESCRIBE) so a stale
/// function in a long-running shell is diagnosable by comparing the comment
/// to `clone --version`.
///
/// Behaviours baked in:
/// - `command clone` bypasses the same-named function (no `$CLONE` env var).
/// - `shell-init` is in the passthrough alongside `-h|--help|-v|--version` so
///   an interactive `clone shell-init zsh` after the function is loaded prints
///   the script instead of trying to `cd` into it.
/// - Destination validated (`-z "$dest" || ! -d "$dest"`) before `cd`.
const ZSH: &str = concat!(
    "# clone - smart git clone (bare-worktree layout) [shell-init ",
    env!("GIT_DESCRIBE"),
    "]\n",
    "# Install: add to your .zshrc -> if hash clone 2>/dev/null; then eval \"$(command clone shell-init zsh)\"; fi\n",
    "clone() {\n",
    "    if [[ \"$1\" == (-h|--help|-v|--version|shell-init) ]]; then\n",
    "        command clone \"$@\"\n",
    "    else\n",
    "        local dest\n",
    "        dest=$(command clone \"$@\") || return $?\n",
    "        if [[ -z \"$dest\" || ! -d \"$dest\" ]]; then\n",
    "            print -u2 -- \"clone: no valid destination returned; staying in $PWD\"\n",
    "            return 1\n",
    "        fi\n",
    "        cd \"$dest\"\n",
    "    fi\n",
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
        other => Err(common::shell::unsupported("clone", SUPPORTED, other)),
    }
}

#[cfg(test)]
mod tests;
