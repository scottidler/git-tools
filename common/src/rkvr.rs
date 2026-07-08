// common - recoverable removal via `rkvr`, behind an injectable [`Remover`] seam.
//
// Both `worktree migrate` and `worktree flatten` perform structural, data-loss-prone
// directory swaps whose removals MUST be recoverable: never a raw `rm -rf`. The
// production remover ([`Rkvr`]) is the single home for that policy - `require`
// probes rkvr in preflight so a missing binary aborts BEFORE any mutation, and
// `rmrf` archives (never deletes) each removed path.
//
// The two verbs take `&dyn Remover` rather than calling rkvr directly, so tests can
// inject a plain filesystem remover and never shell out to the real `rkvr` binary
// (which would need it installed on CI and would spam the real archive with
// throwaway fixtures). Production wires `&Rkvr`, keeping the fail-closed policy
// byte-identical.

use std::path::Path;
use std::process::Command;

use eyre::{Result, bail};
use log::debug;

/// The removal seam threaded through the structural verbs (`migrate`/`flatten`).
/// Production injects [`Rkvr`] (recoverable archive, fail-closed); tests inject a
/// filesystem remover so no `rkvr` process is ever spawned under test.
pub trait Remover {
    /// Refuse to proceed unless recoverable removal is available. Run in preflight
    /// so a missing capability aborts before any mutation.
    fn require(&self) -> Result<()>;
    /// Remove `path` (a missing path is a no-op).
    fn rmrf(&self, path: &Path) -> Result<()>;
}

/// Production remover: routes every removal through `rkvr` so it is archived and
/// recoverable, never a raw non-recoverable delete (the project's hard safety rule).
pub struct Rkvr;

impl Remover for Rkvr {
    /// Refuse to proceed without `rkvr`: a structural collapse/migration's removals
    /// must be recoverable, never a raw non-recoverable delete. A missing binary
    /// aborts before any mutation.
    fn require(&self) -> Result<()> {
        debug!("Rkvr::require: probing for rkvr");
        match Command::new("rkvr").arg("--version").output() {
            Ok(o) if o.status.success() => Ok(()),
            _ => bail!(
                "`rkvr` is required for this operation (its removals must be recoverable); \
                 install it and re-run"
            ),
        }
    }

    /// Remove `path` recoverably via `rkvr rmrf` (archived, recoverable until rkvr
    /// harvests it). `rkvr` presence is enforced by [`Rkvr::require`] in preflight,
    /// so a missing rkvr here is an error, never a silent non-recoverable delete. A
    /// missing path is a no-op.
    fn rmrf(&self, path: &Path) -> Result<()> {
        debug!("Rkvr::rmrf: path={:?}", path);
        if path.symlink_metadata().is_err() {
            debug!("Rkvr::rmrf: {:?} does not exist; no-op", path);
            return Ok(());
        }
        match Command::new("rkvr").arg("rmrf").arg(path).status() {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => bail!("rkvr rmrf {} failed: {}", path.display(), status),
            Err(e) => bail!("rkvr rmrf {} could not run: {}", path.display(), e),
        }
    }
}
