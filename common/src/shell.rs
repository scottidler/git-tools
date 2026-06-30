// common::shell - shared shell-init scaffolding.
//
// Provides the uniform "unsupported shell" error used by every crate that
// emits a shell-init script. The supported-shell list is a caller parameter
// (not a shared global) so each crate's error message reflects its own truth:
// one tool can gain `bash` support before another without the shared code
// lying about what is actually available.

use eyre::{Report, eyre};
use log::debug;

/// Build a uniform "unsupported shell" error.
///
/// * `bin`       - the binary name (e.g. `"clone"`)
/// * `supported` - the shells this binary actually supports (e.g. `&["zsh"]`)
/// * `shell`     - the shell name the caller passed in
///
/// The returned `Report` names the command, echoes the bad shell, and lists the
/// supported set so the user knows what to pass instead.
pub fn unsupported(bin: &str, supported: &[&str], shell: &str) -> Report {
    debug!("unsupported: bin={} shell={} supported={:?}", bin, shell, supported);
    let list = supported.join(", ");
    eyre!("{}: unsupported shell {:?}; supported: {}", bin, shell, list)
}

#[cfg(test)]
mod tests;
