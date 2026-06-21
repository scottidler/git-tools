use eyre::Result;
use std::path::Path;

/// Parse a Git remote URL into `owner/repo` format.
///
/// Thin shim over the host-agnostic [`crate::git::parse_repospec`] (slated for
/// retirement in Phase 6 if unused). Returns `None` on any unparseable input,
/// preserving the original `Option`-returning contract while gaining GitLab /
/// Bitbucket / enterprise support for free.
pub fn parse_git_url(url: &str) -> Option<String> {
    crate::git::parse_repospec(url).ok().map(|spec| spec.to_string())
}

/// Get the repository slug from a path by querying git remote.
///
/// Routes through the shared `git::output` runner so the failure path carries
/// captured stderr like every other call site.
pub fn get_repo_slug_from_path<P: AsRef<Path>>(path: P) -> Result<String> {
    let repo_dir = path.as_ref();

    let url_out = crate::git::output(&["remote", "get-url", "origin"], Some(repo_dir), None)?;
    if !url_out.status.success() {
        eyre::bail!(
            "Failed to get git remote URL from {}: {}",
            repo_dir.display(),
            url_out.stderr.trim()
        );
    }

    let url = url_out.stdout.trim();
    parse_git_url(url).ok_or_else(|| eyre::eyre!("Failed to parse git URL: {}", url))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_url_ssh() {
        let url = "git@github.com:owner/repo.git";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_ssh_no_git() {
        let url = "git@github.com:owner/repo";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_https() {
        let url = "https://github.com/owner/repo.git";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_https_no_git() {
        let url = "https://github.com/owner/repo";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_ssh_protocol() {
        let url = "ssh://git@github.com/owner/repo.git";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_ssh_protocol_no_git() {
        let url = "ssh://git@github.com/owner/repo";
        assert_eq!(parse_git_url(url), Some("owner/repo".to_string()));
    }

    #[test]
    fn test_parse_git_url_invalid() {
        let url = "invalid-url";
        assert_eq!(parse_git_url(url), None);
    }

    #[test]
    fn test_parse_git_url_empty() {
        let url = "";
        assert_eq!(parse_git_url(url), None);
    }

    #[test]
    fn test_get_repo_slug_from_path_non_github() {
        let tmp = tempfile::TempDir::new().unwrap();
        let repo = tmp.path().join("r");
        std::fs::create_dir_all(&repo).unwrap();
        crate::git::run(&["init", "-b", "main"], Some(&repo), None).unwrap();
        crate::git::run(
            &["remote", "add", "origin", "git@gitlab.com:someorg/somerepo.git"],
            Some(&repo),
            None,
        )
        .unwrap();

        assert_eq!(get_repo_slug_from_path(&repo).unwrap(), "someorg/somerepo");
    }
}
