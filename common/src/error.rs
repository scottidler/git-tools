use std::error::Error as StdError;
use std::fmt;

use eyre::EyreHandler;

/// Install an eyre hook that renders errors as a clean Display chain - the
/// message plus any `Caused by:` sources - with NO `Location:` / backtrace
/// section. For these user-facing CLIs the file:line of a `bail!` is noise.
///
/// Safe to call more than once (a second install is ignored), and called by
/// [`crate::log::init`] so every binary gets it without per-`main` wiring.
/// `{:#?}` (alternate Debug) still yields the full structural dump for anyone
/// who explicitly asks.
pub fn install() {
    let _ = eyre::set_hook(Box::new(|_| Box::new(QuietHandler)));
}

struct QuietHandler;

impl EyreHandler for QuietHandler {
    fn debug(&self, error: &(dyn StdError + 'static), f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            return fmt::Debug::fmt(error, f);
        }
        write!(f, "{error}")?;
        let mut source = error.source();
        while let Some(err) = source {
            write!(f, "\n\nCaused by:\n    {err}")?;
            source = err.source();
        }
        Ok(())
    }
}
