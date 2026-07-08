// common — shared config reader for `clone` and `worktree`.
//
// Still INI-backed and still reading the old `clone.cfg` paths (moved verbatim
// from `clone::config`); the YAML migration to a shared `git-tools.yml` lands
// in a later phase.

use std::env;
use std::path::Path;

use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::warn;

/// Read a single value from the `[clone]` section of `clone.cfg`, if the file
/// and key are present. Honors `$CLONE_CFG`, else `~/.config/clone/clone.cfg`.
pub fn clone_cfg_value(key: &str) -> Option<String> {
    let home = env::var("HOME").ok()?;
    let path = env::var("CLONE_CFG").unwrap_or_else(|_| format!("{}/.config/clone/clone.cfg", home));
    if !Path::new(&path).exists() {
        return None;
    }
    let cfg = ini!(&path);
    cfg.get("clone").and_then(|m| m.get(key).cloned().flatten())
}

/// Resolve the per-org transport SSH key from `clone.cfg`.
///
/// Reads `$CLONE_CFG` (or `~/.config/clone/clone.cfg`), looks up the
/// `[org.<org>]` section, falling back to `[org.default]`. Returns `Ok(None)`
/// when no config file is present. Accepts either an `org` or a full
/// `org/repo`; only the leading org component is used.
pub fn find_ssh_key_for_org(repospec: &str) -> Result<Option<String>> {
    let home_dir = env::var("HOME").wrap_err("Failed to get HOME environment variable")?;
    let config_path = env::var("CLONE_CFG").unwrap_or_else(|_| format!("{}/.config/clone/clone.cfg", home_dir));

    if !Path::new(&config_path).exists() {
        warn!("Configuration file not found: {:?}", config_path);
        return Ok(None);
    }

    let cfg = ini!(&config_path);
    if cfg.is_empty() {
        return Err(eyre!("Failed to load configuration file"));
    }

    let org_name = repospec
        .split('/')
        .next()
        .ok_or_else(|| eyre!("Invalid repospec format"))?;
    let section_key = format!("org.{}", org_name);
    let ssh_key_map = cfg
        .get(&section_key)
        .or_else(|| cfg.get("org.default"))
        .ok_or_else(|| eyre!("Configuration section not found"))?;

    let ssh_key = ssh_key_map.get("sshkey").cloned().flatten();

    Ok(ssh_key)
}

#[cfg(test)]
mod tests;
