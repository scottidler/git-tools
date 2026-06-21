use eyre::Result;
use log::LevelFilter;

/// Initialize logging to **stderr** at the given `--log-level` level.
///
/// Deliberately does NOT consult `RUST_LOG` (project rule: `--log-level` is the
/// only knob). Targets stderr, matching the previous `env_logger::init()`
/// behavior for these short-lived list CLIs. Idempotent: uses `try_init`, so it
/// is safe to call from tests without a double-init panic.
pub fn init(level: LevelFilter, project: &str) -> Result<()> {
    let _ = env_logger::Builder::new()
        .filter_level(level)
        .target(env_logger::Target::Stderr)
        .format_timestamp(None)
        .try_init();
    log::debug!("{project}: logging initialized at level {level}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_is_idempotent() {
        // Two calls must not panic (try_init swallows the second).
        assert!(init(LevelFilter::Warn, "common-test").is_ok());
        assert!(init(LevelFilter::Debug, "common-test").is_ok());
    }
}
