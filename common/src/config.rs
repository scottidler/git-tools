// common — shared config reader for `clone` and `worktree`.
//
// YAML-primary at `~/.config/git-tools/git-tools.yml` (or `$GIT_TOOLS_CFG`),
// falling back to the legacy INI `clone.cfg` (`$CLONE_CFG`, else
// `~/.config/clone/clone.cfg`) for back-compat. Both formats deserialize into
// the same in-memory `Config`, so callers stay format-agnostic.
//
// Fail-closed load semantics: the first location whose file EXISTS is THE
// config for this run — its format is fixed by which location supplied it.
// If it exists but fails to parse, that is a loud error; the reader never
// falls through to a lower-precedence file on a parse failure, only on an
// ABSENT file. A missing `default-branch` is a lookup default (not an
// error); a missing/unmatched org returns `None` from the SSH-key lookup
// (not an error, never a hard failure — transport just proceeds with no
// `-i` key).

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use eyre::{Result, WrapErr, eyre};
use ini::ini;
use log::debug;
use serde::Deserialize;

/// Shared git-tools config: deserialized from either YAML (`git-tools.yml`)
/// or, for back-compat, the legacy INI `clone.cfg`.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Config {
    /// Fallback default branch, used only when a remote's default branch
    /// can't be detected. Absent means "use the caller's built-in default".
    #[serde(default)]
    pub default_branch: Option<String>,
    /// Per-org SSH key configuration, keyed by org name. A literal `default`
    /// entry is the catch-all for orgs with no explicit entry.
    #[serde(default)]
    pub orgs: HashMap<String, OrgConfig>,
}

/// Per-org config. Today this is just the SSH key used for transport.
#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OrgConfig {
    #[serde(default)]
    pub sshkey: Option<String>,
}

/// File format, fixed by which location supplied the config.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Yaml,
    Ini,
}

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to
/// `$HOME/.config`. Deliberately NOT `dirs::config_dir()`: that only honors
/// `$XDG_CONFIG_HOME` on Linux, resolving to `~/Library/...` on macOS instead.
pub fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

/// The candidate config locations, in precedence order. A location whose env
/// override isn't set (and which has no fixed default path) is omitted, not
/// treated as an empty/existing candidate.
fn candidates() -> Vec<(PathBuf, Format)> {
    let mut out = Vec::new();
    if let Ok(path) = env::var("GIT_TOOLS_CFG") {
        out.push((PathBuf::from(path), Format::Yaml));
    }
    if let Some(dir) = xdg_config_dir() {
        out.push((dir.join("git-tools").join("git-tools.yml"), Format::Yaml));
    }
    if let Ok(path) = env::var("CLONE_CFG") {
        out.push((PathBuf::from(path), Format::Ini));
    }
    if let Ok(home) = env::var("HOME") {
        out.push((PathBuf::from(home).join(".config/clone/clone.cfg"), Format::Ini));
    }
    out
}

/// Resolve and load the first-existing config location. `Ok(None)` means no
/// candidate location has a file present (fall back to lookup defaults
/// everywhere). A present-but-unparseable file is a loud `Err` — no
/// fall-through past a broken higher-precedence file.
fn load() -> Result<Option<Config>> {
    debug!(
        "common::config::load: resolving config across {} candidate locations",
        candidates().len()
    );
    for (path, format) in candidates() {
        if !path.is_file() {
            continue;
        }
        debug!("common::config::load: loading {:?} as {:?}", path, format);
        let config = match format {
            Format::Yaml => parse_yaml(&path),
            Format::Ini => parse_ini(&path),
        }
        .wrap_err_with(|| {
            format!(
                "failed to load config file {:?} (existing higher-precedence config is never skipped on a parse error)",
                path
            )
        })?;
        debug!("common::config::load: loaded config from {:?}", path);
        return Ok(Some(config));
    }
    debug!("common::config::load: no config file present at any candidate location");
    Ok(None)
}

fn parse_yaml(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path).wrap_err_with(|| format!("failed to read {:?}", path))?;
    serde_yaml::from_str(&raw).wrap_err_with(|| format!("failed to parse YAML config {:?}", path))
}

fn parse_ini(path: &Path) -> Result<Config> {
    let path_str = path
        .to_str()
        .ok_or_else(|| eyre!("config path {:?} is not valid UTF-8", path))?;
    let raw = ini!(safe path_str).map_err(|e| eyre!("failed to parse INI config {:?}: {}", path, e))?;

    let default_branch = raw
        .get("clone")
        .and_then(|section| section.get("default").cloned().flatten());

    let mut orgs = HashMap::new();
    for (section_name, section) in &raw {
        let Some(org) = section_name.strip_prefix("org.") else {
            continue;
        };
        let sshkey = section.get("sshkey").cloned().flatten();
        orgs.insert(org.to_string(), OrgConfig { sshkey });
    }

    Ok(Config { default_branch, orgs })
}

/// The configured default-branch fallback, or `Ok(None)` when unset or no
/// config file is present (the caller then applies its own built-in default).
pub fn default_branch() -> Result<Option<String>> {
    debug!("common::config::default_branch");
    let result = load()?.and_then(|c| c.default_branch);
    debug!("common::config::default_branch: resolved={:?}", result);
    Ok(result)
}

/// Resolve the per-org transport SSH key. Looks up `orgs.<org>`, falling back
/// to `orgs.default`. Accepts either an `org` or a full `org/repo`; only the
/// leading org component is used. Returns `Ok(None)` when no config is
/// present, or the config is present but has no matching/default org entry —
/// never a hard error for a lookup miss (transport proceeds with no `-i` key).
pub fn find_ssh_key_for_org(repospec: &str) -> Result<Option<String>> {
    debug!("common::config::find_ssh_key_for_org: repospec={}", repospec);
    let org_name = repospec
        .split('/')
        .next()
        .ok_or_else(|| eyre!("Invalid repospec format"))?;
    let config = load()?;
    let key = config.and_then(|c| {
        c.orgs
            .get(org_name)
            .or_else(|| c.orgs.get("default"))
            .and_then(|o| o.sshkey.clone())
    });
    debug!(
        "common::config::find_ssh_key_for_org: org={} resolved={:?}",
        org_name, key
    );
    Ok(key)
}

#[cfg(test)]
mod tests;
