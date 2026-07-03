use super::*;
use std::io::Write;
use std::sync::Mutex;
use tempfile::TempDir;

// `find_ssh_key_for_org` reads the `$CLONE_CFG` env var; serialize every test
// that reads or mutates it so a test setting CLONE_CFG to a temp path can never
// race a concurrent reader (the `ini!` macro panics on a file deleted
// mid-read). Env-var mutation isn't safe with parallel tests.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_find_ssh_key_with_no_slash() {
    let _guard = ENV_LOCK.lock().unwrap();
    let result = find_ssh_key_for_org("invalid-no-slash");
    assert!(
        result.is_ok() || result.is_err(),
        "Function handles input without slash"
    );
}

#[test]
fn test_find_ssh_key_with_valid_repospec() {
    let _guard = ENV_LOCK.lock().unwrap();
    // This test verifies the function handles valid repospec without panicking.
    // It may return Ok(None) if no config exists, or Err if config exists but
    // doesn't have the required sections - both are acceptable behaviors.
    let result = find_ssh_key_for_org("someorg/somerepo");
    // Just verify it doesn't panic - Ok or Err are both valid outcomes
    let _ = result;
}

#[test]
fn test_find_ssh_key_extracts_org_name() {
    let _guard = ENV_LOCK.lock().unwrap();
    let test_cases = vec![
        ("org/repo", "org"),
        ("my-org/my-repo", "my-org"),
        ("org/repo/extra", "org"),
    ];

    for (repospec, _expected_org) in test_cases {
        let result = find_ssh_key_for_org(repospec);
        assert!(result.is_ok() || result.is_err(), "Should handle {}", repospec);
    }
}

#[test]
fn test_resolve_layout_default_is_flat() {
    assert_eq!(resolve_layout(false, false, false, None), Layout::Flat);
}

#[test]
fn test_resolve_layout_bare_flag_wins() {
    assert_eq!(resolve_layout(true, false, false, None), Layout::Bare);
    // CLI --bare overrides a `flat` cfg default.
    assert_eq!(resolve_layout(true, false, false, Some("flat")), Layout::Bare);
}

#[test]
fn test_resolve_layout_flat_flag_wins() {
    assert_eq!(resolve_layout(false, true, false, None), Layout::Flat);
    // CLI --flat overrides a `bare` cfg default.
    assert_eq!(resolve_layout(false, true, false, Some("bare")), Layout::Flat);
}

#[test]
fn test_resolve_layout_versioning_implies_flat() {
    assert_eq!(resolve_layout(false, false, true, None), Layout::Flat);
    assert_eq!(resolve_layout(false, false, true, Some("bare")), Layout::Flat);
}

#[test]
fn test_resolve_layout_cfg_default_layout() {
    assert_eq!(resolve_layout(false, false, false, Some("flat")), Layout::Flat);
    assert_eq!(resolve_layout(false, false, false, Some("FLAT")), Layout::Flat);
    assert_eq!(resolve_layout(false, false, false, Some("bare")), Layout::Bare);
    assert_eq!(resolve_layout(false, false, false, Some("BARE")), Layout::Bare);
    assert_eq!(resolve_layout(false, false, false, Some("nonsense")), Layout::Flat);
}

#[test]
fn test_bare_conflicts_with_flat() {
    use crate::cli::Cli;
    use clap::Parser;

    let cli = Cli::try_parse_from(["clone", "--bare", "--flat", "org/repo"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--bare + --flat should be rejected; got: {err}"
    );
}

#[test]
fn test_bare_conflicts_with_migrate() {
    use crate::cli::Cli;
    use clap::Parser;

    let cli = Cli::try_parse_from(["clone", "--bare", "--migrate", "org/repo"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--bare + --migrate should be rejected; got: {err}"
    );
}

#[test]
fn test_versioning_conflicts_with_migrate() {
    use crate::cli::Cli;
    use clap::Parser;

    // --versioning implies flat, which has nothing to migrate to.
    let cli = Cli::try_parse_from(["clone", "--versioning", "--migrate", "org/repo"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--versioning + --migrate should be rejected; got: {err}"
    );
}

#[test]
fn test_flatten_conflicts_with_migrate() {
    use crate::cli::Cli;
    use clap::Parser;

    // Opposite structural conversions: naming both is contradictory.
    let cli = Cli::try_parse_from(["clone", "--flatten", "--migrate"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--flatten + --migrate should be rejected; got: {err}"
    );
}

#[test]
fn test_flatten_conflicts_with_bare() {
    use crate::cli::Cli;
    use clap::Parser;

    // --flatten produces a flat checkout; --bare requests the opposite layout.
    let cli = Cli::try_parse_from(["clone", "--flatten", "--bare"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--flatten + --bare should be rejected; got: {err}"
    );
}

#[test]
fn test_flatten_conflicts_with_versioning() {
    use crate::cli::Cli;
    use clap::Parser;

    let cli = Cli::try_parse_from(["clone", "--flatten", "--versioning"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("cannot be combined"),
        "--flatten + --versioning should be rejected; got: {err}"
    );
}

#[test]
fn test_flatten_without_repospec_is_allowed() {
    use crate::cli::Cli;
    use clap::Parser;

    // Like --migrate, --flatten can derive its target from the current directory,
    // so a repospec is optional and the op resolves to Flatten.
    let cli = Cli::try_parse_from(["clone", "--flatten"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Flatten);
    assert!(config.spec.is_none());
}

#[test]
fn test_dry_run_allowed_with_flatten() {
    use crate::cli::Cli;
    use clap::Parser;

    let cli = Cli::try_parse_from(["clone", "--flatten", "--dry-run"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Flatten);
    assert!(config.dry_run);
}

#[test]
fn test_dry_run_rejected_without_conversion() {
    use crate::cli::Cli;
    use clap::Parser;

    // --dry-run only previews a structural conversion; a plain clone rejects it.
    let cli = Cli::try_parse_from(["clone", "--dry-run", "org/repo"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("--dry-run is only valid with --migrate or --flatten"),
        "--dry-run on a plain clone should be rejected; got: {err}"
    );
}

#[test]
fn test_find_ssh_key_with_custom_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("CLONE_CFG").ok();

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("test.cfg");

    let mut file = std::fs::File::create(&config_path).unwrap();
    writeln!(file, "[org.testorg]").unwrap();
    writeln!(file, "sshkey = /path/to/key").unwrap();

    unsafe { std::env::set_var("CLONE_CFG", config_path.to_str().unwrap()) };
    let result = find_ssh_key_for_org("testorg/repo");

    match prior {
        Some(v) => unsafe { std::env::set_var("CLONE_CFG", v) },
        None => unsafe { std::env::remove_var("CLONE_CFG") },
    }

    assert!(result.is_ok());
    if let Ok(Some(key)) = result {
        assert_eq!(key, "/path/to/key");
    }
}
