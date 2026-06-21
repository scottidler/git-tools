use std::fmt;

use eyre::{Result, eyre};

/// A parsed repository specification in `org/repo` form.
///
/// Two-component by design and faithful to the on-disk `~/repos/<org>/<repo>`
/// invariant: only the first two path components are retained (a
/// `gitlab.com/org/team/sub/repo` URL maps to `org=org, repo=team`). Deeper
/// GitLab subgroup support is a pre-existing limitation, out of scope here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSpec {
    pub org: String,
    pub repo: String,
}

impl fmt::Display for RepoSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.org, self.repo)
    }
}

/// Parse a repository specification from various formats into `org/repo`.
///
/// Supported formats:
/// - `org/repo` - Simple format (pass through)
/// - `https://github.com/org/repo` or `https://github.com/org/repo.git` - HTTPS URL
/// - `git@github.com:org/repo` or `git@github.com:org/repo.git` - SCP-style SSH
/// - `ssh://git@github.com/org/repo` or `ssh://git@github.com/org/repo.git` - SSH URL
/// - `git://github.com/org/repo` or `git://github.com/org/repo.git` - Git protocol URL
///
/// Host-agnostic: GitLab, Bitbucket, and enterprise hosts work identically.
pub fn parse_repospec(input: &str) -> Result<RepoSpec> {
    let input = input.trim();

    if input.is_empty() {
        return Err(eyre!("Empty repository specification"));
    }

    // Remove trailing .git if present
    let input = input.strip_suffix(".git").unwrap_or(input);

    // HTTPS URL: https://github.com/org/repo
    if input.starts_with("https://") || input.starts_with("http://") {
        let without_protocol = input
            .strip_prefix("https://")
            .or_else(|| input.strip_prefix("http://"))
            .unwrap_or(input);
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid HTTPS URL: missing path"));
    }

    // SSH URL: ssh://git@github.com/org/repo
    if input.starts_with("ssh://") {
        let without_protocol = input.strip_prefix("ssh://").unwrap_or(input);
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid SSH URL: missing path"));
    }

    // Git protocol URL: git://github.com/org/repo
    if input.starts_with("git://") {
        let without_protocol = input.strip_prefix("git://").unwrap_or(input);
        let parts: Vec<&str> = without_protocol.splitn(2, '/').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid git URL: missing path"));
    }

    // SCP-style SSH: git@github.com:org/repo
    if input.contains('@') && input.contains(':') {
        let parts: Vec<&str> = input.splitn(2, ':').collect();
        if parts.len() == 2 {
            return extract_org_repo_from_path(parts[1]);
        }
        return Err(eyre!("Invalid SCP-style URL: missing colon separator"));
    }

    // Simple org/repo format - validate it has a slash with content on both sides
    if input.contains('/') {
        let parts: Vec<&str> = input.splitn(2, '/').collect();
        if parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() {
            let org = parts[0];
            let repo = parts[1];
            if !org.contains(':') && !repo.contains(':') {
                return extract_org_repo_from_path(input);
            }
        }
    }

    Err(eyre!(
        "Invalid repository specification: '{}'. Expected formats:\n\
         - org/repo\n\
         - https://github.com/org/repo\n\
         - git@github.com:org/repo\n\
         - ssh://git@github.com/org/repo\n\
         - git://github.com/org/repo",
        input
    ))
}

/// Extract `org/repo` from a path, handling extra path components.
fn extract_org_repo_from_path(path: &str) -> Result<RepoSpec> {
    let path = path.strip_suffix(".git").unwrap_or(path);
    let path = path.trim_start_matches('/');

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return Err(eyre!("Invalid path: expected org/repo format, got '{}'", path));
    }

    let org = parts[0];
    let repo = parts[1];

    if org.is_empty() || repo.is_empty() {
        return Err(eyre!("Invalid path: org or repo is empty"));
    }

    Ok(RepoSpec {
        org: org.to_string(),
        repo: repo.to_string(),
    })
}

/// Slugify a branch name to lowercase-hyphenated form (per `general.md`).
///
/// Lowercases, collapses any run of non-alphanumeric characters (slashes,
/// spaces, dots) into a single hyphen, and trims leading/trailing hyphens.
/// `Feature/Foo Bar` -> `feature-foo-bar`.
pub fn slugify_branch(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut prev_hyphen = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_hyphen = false;
        } else if !prev_hyphen {
            slug.push('-');
            prev_hyphen = true;
        }
    }
    slug.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(input: &str) -> String {
        parse_repospec(input).unwrap().to_string()
    }

    #[test]
    fn test_parse_simple_org_repo() {
        assert_eq!(slug("scottidler/gx"), "scottidler/gx");
        assert_eq!(slug("otto-rs/otto"), "otto-rs/otto");
        assert_eq!(slug("tatari-tv/philo"), "tatari-tv/philo");
    }

    #[test]
    fn test_parse_https_url() {
        assert_eq!(slug("https://github.com/scottidler/gx"), "scottidler/gx");
        assert_eq!(slug("https://github.com/otto-rs/otto"), "otto-rs/otto");
        assert_eq!(slug("https://github.com/tatari-tv/philo"), "tatari-tv/philo");
    }

    #[test]
    fn test_parse_https_url_with_git_suffix() {
        assert_eq!(slug("https://github.com/scottidler/gx.git"), "scottidler/gx");
        assert_eq!(slug("https://github.com/otto-rs/otto.git"), "otto-rs/otto");
    }

    #[test]
    fn test_parse_http_url() {
        assert_eq!(slug("http://github.com/scottidler/gx"), "scottidler/gx");
    }

    #[test]
    fn test_parse_ssh_url() {
        assert_eq!(slug("ssh://git@github.com/scottidler/gx"), "scottidler/gx");
        assert_eq!(slug("ssh://git@github.com/otto-rs/otto"), "otto-rs/otto");
    }

    #[test]
    fn test_parse_ssh_url_with_git_suffix() {
        assert_eq!(slug("ssh://git@github.com/scottidler/gx.git"), "scottidler/gx");
    }

    #[test]
    fn test_parse_git_protocol_url() {
        assert_eq!(slug("git://github.com/scottidler/gx"), "scottidler/gx");
        assert_eq!(slug("git://github.com/otto-rs/otto.git"), "otto-rs/otto");
    }

    #[test]
    fn test_parse_scp_style_ssh() {
        assert_eq!(slug("git@github.com:scottidler/gx"), "scottidler/gx");
        assert_eq!(slug("git@github.com:otto-rs/otto"), "otto-rs/otto");
        assert_eq!(slug("git@github.com:tatari-tv/philo"), "tatari-tv/philo");
    }

    #[test]
    fn test_parse_scp_style_ssh_with_git_suffix() {
        assert_eq!(slug("git@github.com:scottidler/gx.git"), "scottidler/gx");
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(slug("  scottidler/gx  "), "scottidler/gx");
        assert_eq!(slug("\thttps://github.com/scottidler/gx\n"), "scottidler/gx");
    }

    #[test]
    fn test_parse_different_hosts() {
        // GitLab
        assert_eq!(slug("https://gitlab.com/someorg/somerepo"), "someorg/somerepo");
        assert_eq!(slug("git@gitlab.com:someorg/somerepo.git"), "someorg/somerepo");
        // Bitbucket
        assert_eq!(slug("https://bitbucket.org/someorg/somerepo"), "someorg/somerepo");
        // Enterprise GitHub
        assert_eq!(
            slug("https://github.enterprise.com/someorg/somerepo"),
            "someorg/somerepo"
        );
    }

    #[test]
    fn test_parse_empty_input() {
        assert!(parse_repospec("").is_err());
        assert!(parse_repospec("   ").is_err());
    }

    #[test]
    fn test_parse_invalid_formats() {
        assert!(parse_repospec("justrepo").is_err());
        assert!(parse_repospec("/repo").is_err());
        assert!(parse_repospec("org/").is_err());
        assert!(parse_repospec("https://github.com").is_err());
        assert!(parse_repospec("https://github.com/").is_err());
    }

    #[test]
    fn test_parse_url_with_extra_path_components() {
        assert_eq!(slug("https://github.com/scottidler/gx/tree/main"), "scottidler/gx");
        assert_eq!(
            slug("https://github.com/scottidler/gx/blob/main/README.md"),
            "scottidler/gx"
        );
    }

    #[test]
    fn test_repospec_fields_and_display() {
        let spec = parse_repospec("git@gitlab.com:myorg/myrepo.git").unwrap();
        assert_eq!(spec.org, "myorg");
        assert_eq!(spec.repo, "myrepo");
        assert_eq!(spec.to_string(), "myorg/myrepo");
    }

    #[test]
    fn test_slugify_branch() {
        assert_eq!(slugify_branch("Feature/Foo Bar"), "feature-foo-bar");
        assert_eq!(slugify_branch("release/1.2"), "release-1-2");
        assert_eq!(slugify_branch("add-auth"), "add-auth");
        assert_eq!(slugify_branch("Add Auth"), "add-auth");
        assert_eq!(slugify_branch("--weird//name--"), "weird-name");
    }
}
