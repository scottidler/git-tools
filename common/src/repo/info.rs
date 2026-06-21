use eyre::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::git;

/// Information about a discovered Git repository
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoInfo {
    /// The filesystem path to the repository working tree.
    ///
    /// For a flat clone this is the checkout root; for a bare container this is
    /// the canonical default-branch worktree (`<container>/<default-branch>`,
    /// always present) so consumers always get a real working tree.
    pub path: PathBuf,
    /// The repository slug in "owner/repo" format
    pub slug: String,
    /// The worktree name, when this row represents a specific worktree.
    ///
    /// `None` for flat clones and for the default logical row of a bare
    /// container. Only set when a caller opts into per-worktree enumeration.
    #[serde(default)]
    pub worktree: Option<String>,
}

impl RepoInfo {
    /// Create a new RepoInfo with the given path and slug (no specific worktree).
    pub fn new(path: PathBuf, slug: String) -> Self {
        Self {
            path,
            slug,
            worktree: None,
        }
    }

    /// Create a RepoInfo tagged with a specific worktree name.
    pub fn with_worktree(path: PathBuf, slug: String, worktree: Option<String>) -> Self {
        Self { path, slug, worktree }
    }

    /// Create a RepoInfo by discovering repository information from a path
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let (repo_root, slug) = find_repo_root_and_slug(path)?;
        Ok(Self::new(repo_root, slug))
    }

    /// Build the default logical-repo row for a bare container.
    ///
    /// `path` is the canonical default-branch worktree (always present),
    /// `slug` is `org/repo`, and `worktree` is `None`.
    pub fn from_bare_container<P: AsRef<Path>>(container: P) -> Result<Self> {
        let container = container.as_ref();
        let branch = default_branch(container)?;
        let path = container.join(&branch);
        let slug = container_slug(container);
        Ok(Self::new(path, slug))
    }

    /// Enumerate every real worktree of a bare container as its own row
    /// (the bare repo entry itself is skipped). Each row carries
    /// `worktree = Some(name)` and `path` pointing at that worktree.
    pub fn worktrees_of<P: AsRef<Path>>(container: P) -> Result<Vec<RepoInfo>> {
        let container = container.as_ref();
        let slug = container_slug(container);

        let out = git::output(&["worktree", "list", "--porcelain"], Some(container), None)?;
        if !out.status.success() {
            eyre::bail!(
                "git worktree list failed in '{}': {}",
                container.display(),
                out.stderr.trim()
            );
        }

        let mut infos = Vec::new();
        for block in out.stdout.split("\n\n") {
            let mut wt_path: Option<PathBuf> = None;
            let mut branch: Option<String> = None;
            let mut is_bare = false;

            for line in block.lines() {
                if let Some(p) = line.strip_prefix("worktree ") {
                    wt_path = Some(PathBuf::from(p.trim()));
                } else if line.trim() == "bare" {
                    is_bare = true;
                } else if let Some(b) = line.strip_prefix("branch ") {
                    branch = Some(b.trim().trim_start_matches("refs/heads/").to_string());
                }
            }

            if is_bare {
                continue;
            }
            if let Some(path) = wt_path {
                let name = branch.unwrap_or_else(|| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("detached")
                        .to_string()
                });
                infos.push(RepoInfo::with_worktree(path, slug.clone(), Some(name)));
            }
        }

        Ok(infos)
    }
}

/// Determine a bare container's default branch.
///
/// A `git clone --bare` sets the bare repo's `HEAD` to the remote default
/// branch, so `symbolic-ref --short HEAD` yields it directly; fall back to the
/// remote head ref. Never hardcodes `main`.
pub fn default_branch(container: &Path) -> Result<String> {
    let out = git::output(&["symbolic-ref", "--short", "HEAD"], Some(container), None)?;
    if out.status.success() {
        let branch = out.stdout.trim().to_string();
        if !branch.is_empty() {
            return Ok(branch);
        }
    }

    let out = git::output(
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        Some(container),
        None,
    )?;
    if out.status.success() {
        let branch = out.stdout.trim().trim_start_matches("origin/").to_string();
        if !branch.is_empty() {
            return Ok(branch);
        }
    }

    eyre::bail!(
        "could not determine default branch for bare container '{}'",
        container.display()
    )
}

/// Slug for a bare container: parse `origin` if available, else derive from the
/// container's own `<org>/<repo>` path position.
fn container_slug(container: &Path) -> String {
    if let Ok(out) = git::output(&["remote", "get-url", "origin"], Some(container), None)
        && out.status.success()
        && let Some(slug) = git::parse_git_url(out.stdout.trim())
    {
        return slug;
    }
    slug_from_path(container)
}

/// Finds the repo root (via `git rev-parse`) and parses `origin` → `org/repo`.
fn find_repo_root_and_slug<P: AsRef<Path>>(path: P) -> Result<(PathBuf, String)> {
    let repo_dir = path.as_ref();

    let root = git::output(&["rev-parse", "--show-toplevel"], Some(repo_dir), None)?;
    if !root.status.success() {
        eyre::bail!("Not inside a Git repository at '{}'", repo_dir.display());
    }
    let repo_root = PathBuf::from(root.stdout.trim_end().to_string());

    let url_out = git::output(&["remote", "get-url", "origin"], Some(repo_dir), None)?;
    let url = url_out.stdout.trim().to_string();
    let slug = git::parse_git_url(&url).unwrap_or_else(|| slug_from_path(&repo_root));

    Ok((repo_root, slug))
}

/// Derive a slug from the filesystem path as a fallback.
/// Uses the last two path components (e.g., `/home/user/repos/org/repo` -> `org/repo`).
fn slug_from_path(path: &Path) -> String {
    let components: Vec<&str> = path.components().filter_map(|c| c.as_os_str().to_str()).collect();
    let len = components.len();
    if len >= 2 {
        format!("{}/{}", components[len - 2], components[len - 1])
    } else if len == 1 {
        format!("unknown/{}", components[0])
    } else {
        "unknown/unknown".into()
    }
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
        assert_eq!(info.worktree, None);
    }

    #[test]
    fn test_repo_info_with_worktree() {
        let info = RepoInfo::with_worktree(
            PathBuf::from("/test/repo/feature"),
            "owner/repo".to_string(),
            Some("feature".to_string()),
        );
        assert_eq!(info.worktree.as_deref(), Some("feature"));
    }
}
