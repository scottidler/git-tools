// common - recoverable removal via `rkvr`.
//
// Both `clone --migrate` and `clone --flatten` perform structural, data-loss-prone
// directory swaps whose removals MUST be recoverable: never a raw `rm -rf`. These
// two helpers are the single home for that policy - preflight `require`s rkvr so a
// missing binary aborts BEFORE any mutation, and `rmrf` archives (never deletes)
// each removed path.

use std::path::Path;
use std::process::Command;

use eyre::{Result, bail};
use log::debug;

/// Refuse to proceed without `rkvr`: a structural collapse/migration's removals
/// must be recoverable, never a raw non-recoverable delete (the project's hard
/// safety rule). Run in preflight so a missing binary aborts before any mutation.
pub fn require() -> Result<()> {
    debug!("rkvr::require: probing for rkvr");
    match Command::new("rkvr").arg("--version").output() {
        Ok(o) if o.status.success() => Ok(()),
        _ => bail!(
            "`rkvr` is required for this operation (its removals must be recoverable); \
             install it and re-run"
        ),
    }
}

/// Remove `path` recoverably via `rkvr rmrf` (archived, recoverable until rkvr
/// harvests it). `rkvr` presence is enforced by [`require`] in preflight, so a
/// missing rkvr here is an error, never a silent non-recoverable delete. A missing
/// path is a no-op.
pub fn rmrf(path: &Path) -> Result<()> {
    debug!("rkvr::rmrf: path={:?}", path);
    if path.symlink_metadata().is_err() {
        debug!("rkvr::rmrf: {:?} does not exist; no-op", path);
        return Ok(());
    }
    match Command::new("rkvr").arg("rmrf").arg(path).status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => bail!("rkvr rmrf {} failed: {}", path.display(), status),
        Err(e) => bail!("rkvr rmrf {} could not run: {}", path.display(), e),
    }
}
