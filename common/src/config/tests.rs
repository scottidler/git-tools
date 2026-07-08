use super::*;
use std::io::Write;
use std::sync::Mutex;
use tempfile::TempDir;

// Every test here reads or mutates process-wide env vars (`GIT_TOOLS_CFG`,
// `XDG_CONFIG_HOME`, `CLONE_CFG`, `HOME`) that `common::config` resolves the
// candidate locations from. Serialize them behind one lock so no test's
// override leaks into a concurrently-running one.
static ENV_LOCK: Mutex<()> = Mutex::new(());

const ENV_VARS: &[&str] = &["GIT_TOOLS_CFG", "XDG_CONFIG_HOME", "CLONE_CFG", "HOME"];

/// Snapshot the config-relevant env vars, clear them all, and return the
/// snapshot so the caller can restore it. Clearing first means each test
/// starts from "no candidate location resolves" and opts in only to the
/// locations it's exercising.
fn clear_env() -> Vec<(&'static str, Option<String>)> {
    ENV_VARS
        .iter()
        .map(|&name| {
            let prior = env::var(name).ok();
            unsafe { env::remove_var(name) };
            (name, prior)
        })
        .collect()
}

fn restore_env(saved: Vec<(&'static str, Option<String>)>) {
    for (name, value) in saved {
        match value {
            Some(v) => unsafe { env::set_var(name, v) },
            None => unsafe { env::remove_var(name) },
        }
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(contents.as_bytes()).unwrap();
}

#[test]
fn test_xdg_config_dir_honors_env_and_falls_back() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let dir = TempDir::new().unwrap();
    unsafe { env::set_var("XDG_CONFIG_HOME", dir.path()) };
    assert_eq!(xdg_config_dir().as_deref(), Some(dir.path()));

    unsafe { env::remove_var("XDG_CONFIG_HOME") };
    unsafe { env::set_var("HOME", "/home/nonexistent-test-home") };
    assert_eq!(
        xdg_config_dir(),
        Some(PathBuf::from("/home/nonexistent-test-home/.config"))
    );

    restore_env(saved);
}

#[test]
fn test_no_config_present_returns_none_everywhere() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };
    unsafe { env::set_var("HOME", home.path()) };

    assert_eq!(default_branch().unwrap(), None);
    assert_eq!(find_ssh_key_for_org("someorg/somerepo").unwrap(), None);

    restore_env(saved);
}

#[test]
fn test_yaml_at_xdg_path_wins_over_ini_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "default-branch: from-yaml\n",
    );
    write_file(
        &home.path().join(".config/clone/clone.cfg"),
        "[clone]\ndefault = from-ini\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };
    unsafe { env::set_var("HOME", home.path()) };

    assert_eq!(default_branch().unwrap().as_deref(), Some("from-yaml"));

    restore_env(saved);
}

#[test]
fn test_explicit_override_wins_over_xdg_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    let override_dir = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "default-branch: from-xdg\n",
    );
    let override_path = override_dir.path().join("override.yml");
    write_file(&override_path, "default-branch: from-override\n");

    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };
    unsafe { env::set_var("GIT_TOOLS_CFG", &override_path) };

    assert_eq!(default_branch().unwrap().as_deref(), Some("from-override"));

    restore_env(saved);
}

#[test]
fn test_absent_higher_file_falls_through_to_ini() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    // XDG_CONFIG_HOME points at a real dir, but no git-tools.yml exists in it:
    // the YAML location is ABSENT (not malformed), so fall-through to the INI
    // location must happen.
    let xdg = TempDir::new().unwrap();
    let cfg_dir = TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("clone.cfg");
    write_file(&cfg_path, "[clone]\ndefault = from-ini\n");

    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };
    unsafe { env::set_var("CLONE_CFG", &cfg_path) };

    assert_eq!(default_branch().unwrap().as_deref(), Some("from-ini"));

    restore_env(saved);
}

#[test]
fn test_malformed_higher_precedence_file_is_loud_error_not_fallthrough() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    // The GIT_TOOLS_CFG file EXISTS but is malformed YAML. A valid, loadable
    // INI file sits at the next-precedence location. If the reader
    // incorrectly fell through past the broken file, this would resolve to
    // Some("from-ini") instead of erroring -- that is exactly the bug this
    // test bites.
    let override_dir = TempDir::new().unwrap();
    let cfg_dir = TempDir::new().unwrap();
    let override_path = override_dir.path().join("broken.yml");
    write_file(&override_path, "default-branch: [this is not, valid: yaml\n");
    let cfg_path = cfg_dir.path().join("clone.cfg");
    write_file(&cfg_path, "[clone]\ndefault = from-ini\n");

    unsafe { env::set_var("GIT_TOOLS_CFG", &override_path) };
    unsafe { env::set_var("CLONE_CFG", &cfg_path) };

    let result = default_branch();
    assert!(
        result.is_err(),
        "a malformed higher-precedence file must be a loud error"
    );

    restore_env(saved);
}

#[test]
fn test_unknown_yaml_field_is_rejected() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "default-branch: main\ntypo-field: oops\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };

    let result = default_branch();
    assert!(
        result.is_err(),
        "an unknown top-level field must be a loud error, not silent data loss"
    );

    restore_env(saved);
}

#[test]
fn test_missing_default_branch_key_falls_back_to_lookup_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "orgs:\n  tatari-tv:\n    sshkey: ~/.ssh/tatari\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };

    assert_eq!(default_branch().unwrap(), None);

    restore_env(saved);
}

#[test]
fn test_ssh_key_matches_explicit_org() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "orgs:\n  tatari-tv:\n    sshkey: /path/to/tatari-key\n  default:\n    sshkey: /path/to/default-key\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };

    assert_eq!(
        find_ssh_key_for_org("tatari-tv/some-repo").unwrap().as_deref(),
        Some("/path/to/tatari-key")
    );

    restore_env(saved);
}

#[test]
fn test_ssh_key_falls_back_to_default_org_entry() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "orgs:\n  tatari-tv:\n    sshkey: /path/to/tatari-key\n  default:\n    sshkey: /path/to/default-key\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };

    assert_eq!(
        find_ssh_key_for_org("some-unlisted-org/some-repo").unwrap().as_deref(),
        Some("/path/to/default-key")
    );

    restore_env(saved);
}

#[test]
fn test_missing_or_unmatched_org_returns_none_not_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let xdg = TempDir::new().unwrap();
    write_file(
        &xdg.path().join("git-tools/git-tools.yml"),
        "orgs:\n  tatari-tv:\n    sshkey: /path/to/tatari-key\n",
    );
    unsafe { env::set_var("XDG_CONFIG_HOME", xdg.path()) };

    // No "default" entry and no match for "some-unlisted-org" -- must be a
    // permissive None, never a hard error.
    let result = find_ssh_key_for_org("some-unlisted-org/some-repo");
    assert_eq!(result.unwrap(), None);

    restore_env(saved);
}

#[test]
fn test_ini_fallback_resolves_ssh_key_from_legacy_format() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let cfg_dir = TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("clone.cfg");
    write_file(&cfg_path, "[org.testorg]\nsshkey = /path/to/key\n");
    unsafe { env::set_var("CLONE_CFG", &cfg_path) };

    let result = find_ssh_key_for_org("testorg/repo").unwrap();
    assert_eq!(result.as_deref(), Some("/path/to/key"));

    restore_env(saved);
}

#[test]
fn test_ini_default_org_section_used_as_fallback() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let cfg_dir = TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("clone.cfg");
    write_file(&cfg_path, "[org.default]\nsshkey = /path/to/fallback-key\n");
    unsafe { env::set_var("CLONE_CFG", &cfg_path) };

    let result = find_ssh_key_for_org("unlisted/repo").unwrap();
    assert_eq!(result.as_deref(), Some("/path/to/fallback-key"));

    restore_env(saved);
}

#[test]
fn test_malformed_ini_is_a_loud_error() {
    let _guard = ENV_LOCK.lock().unwrap();
    let saved = clear_env();

    let cfg_dir = TempDir::new().unwrap();
    let cfg_path = cfg_dir.path().join("clone.cfg");
    // An unterminated section header is invalid INI syntax.
    write_file(&cfg_path, "[org.testorg\nsshkey = /path/to/key\n");
    unsafe { env::set_var("CLONE_CFG", &cfg_path) };

    let result = find_ssh_key_for_org("testorg/repo");
    assert!(result.is_err(), "a malformed INI file must be a loud error");

    restore_env(saved);
}
