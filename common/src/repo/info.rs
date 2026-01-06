use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Information about a discovered Git repository
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// The filesystem path to the repository root
    pub path: PathBuf,
    /// The repository slug in "owner/repo" format
    pub slug: String,
}

impl RepoInfo {
    /// Create a new RepoInfo with the given path and slug
    pub fn new(path: PathBuf, slug: String) -> Self {
        Self { path, slug }
    }

    /// Create a RepoInfo by discovering repository information from a path
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let (repo_root, slug) = find_repo_root_and_slug(path)?;
        Ok(Self::new(repo_root, slug))
    }
}

/// Finds the repo root (via `git rev-parse`) and parses `origin` → `org/repo`.
fn find_repo_root_and_slug<P: AsRef<Path>>(path: P) -> Result<(PathBuf, String)> {
    let repo_dir = path.as_ref();

    let root = Command::new("git")
        .current_dir(repo_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("git rev-parse failed")?;
    if !root.status.success() {
        eyre::bail!("Not inside a Git repository at '{}'", repo_dir.display());
    }
    let repo_root = PathBuf::from(String::from_utf8(root.stdout)?.trim_end().to_string());

    let url_out = Command::new("git")
        .current_dir(repo_dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .context("git remote get-url failed")?;
    let url = String::from_utf8(url_out.stdout)?.trim().to_string();
    let slug = crate::git::parse_git_url(&url).unwrap_or_else(|| "unknown/unknown".into());

    Ok((repo_root, slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_info_new() {
        let path = PathBuf::from("/test/repo");
        let slug = "owner/repo".to_string();
        let info = RepoInfo::new(path.clone(), slug.clone());

        assert_eq!(info.path, path);
        assert_eq!(info.slug, slug);
    }
}
