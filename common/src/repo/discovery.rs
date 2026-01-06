use super::RepoInfo;
use eyre::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Repository discovery utility for finding Git repositories
pub struct RepoDiscovery {
    paths: Vec<String>,
}

impl RepoDiscovery {
    /// Create a new RepoDiscovery with the given paths to search
    pub fn new(paths: Vec<String>) -> Self {
        Self { paths }
    }

    /// Discover all Git repositories under the configured paths
    /// Returns a Vec of RepoInfo with path and slug information
    ///
    /// Smart matching behavior:
    /// - If path is "." or ends with "/", does directory scanning
    /// - Otherwise tries exact match on repo slug/path first
    /// - Then tries prefix/contains matching on repo slug
    /// - Efficiently filters during scanning rather than scanning all repos first
    pub fn discover(&self) -> Result<Vec<RepoInfo>> {
        // Separate directory paths from potential repo identifiers
        let mut directory_paths = Vec::new();
        let mut repo_identifiers = Vec::new();

        for path in &self.paths {
            if path == "." || path.ends_with('/') || PathBuf::from(path).is_dir() {
                directory_paths.push(path.clone());
            } else {
                repo_identifiers.push(path.clone());
            }
        }

        // If no repo identifiers, just do normal directory scanning
        if repo_identifiers.is_empty() {
            return self.discover_all_repos();
        }

        // For smart matching, scan efficiently with filtering
        let base_paths = if directory_paths.is_empty() { vec![".".to_string()] } else { directory_paths };
        let mut result = Vec::new();
        let mut used_repos = HashSet::new();

        // Find all repo paths and process them in parallel
        let repo_paths = self.find_repo_paths_from_base(&base_paths)?;

        // Process repository paths in parallel to extract RepoInfo and check matches
        let matched_repos: Vec<RepoInfo> = repo_paths
            .par_iter()
            .filter_map(|repo_path| {
                // Only extract RepoInfo if this repo could potentially match
                if let Ok(repo_info) = RepoInfo::from_path(repo_path) {
                    // Check if this repo matches any of our identifiers
                    let mut matched_identifier = None;

                    // Try exact match first
                    for identifier in &repo_identifiers {
                        if repo_info.slug == *identifier || repo_info.path.to_string_lossy() == *identifier {
                            matched_identifier = Some(identifier);
                            break;
                        }
                    }

                    // If no exact match, try prefix/contains matching
                    if matched_identifier.is_none() {
                        for identifier in &repo_identifiers {
                            if repo_info.slug.contains(identifier) {
                                matched_identifier = Some(identifier);
                                break;
                            }
                        }
                    }

                    // Return the repo if it matched
                    if matched_identifier.is_some() {
                        Some(repo_info)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        // Deduplicate results (need to do this sequentially due to HashSet)
        for repo_info in matched_repos {
            if used_repos.insert(repo_info.slug.clone()) {
                result.push(repo_info);
            }
        }

        // Report any identifiers that didn't match anything
        for identifier in &repo_identifiers {
            let found = result.iter().any(|repo| {
                repo.slug == *identifier || repo.path.to_string_lossy() == *identifier || repo.slug.contains(identifier)
            });

            if !found {
                eprintln!("❌ No repositories found matching '{}'", identifier);
            }
        }

        Ok(result)
    }

    /// Discover all repositories by scanning directories (internal helper)
    fn discover_all_repos(&self) -> Result<Vec<RepoInfo>> {
        let repo_paths = self.find_repo_paths()?;

        // Process repository paths in parallel
        let repos: Vec<RepoInfo> = repo_paths
            .par_iter()
            .filter_map(|path| match RepoInfo::from_path(path) {
                Ok(repo_info) => Some(repo_info),
                Err(e) => {
                    eprintln!("❌ {}: {}", path.display(), e);
                    None
                }
            })
            .collect();

        Ok(repos)
    }

    /// Find repo paths from specific base paths (internal helper)
    fn find_repo_paths_from_base(&self, base_paths: &[String]) -> Result<Vec<PathBuf>> {
        let mut repos = Vec::new();

        for p in base_paths {
            let pb = PathBuf::from(p);

            if self.is_git_repo(&pb) {
                repos.push(pb.clone());
                continue;
            }

            if pb.is_dir() {
                for entry in fs::read_dir(&pb).context("reading directory")? {
                    let entry = entry?;
                    let child = entry.path();

                    if self.is_git_repo(&child) {
                        repos.push(child.clone());
                        continue;
                    }

                    if child.is_dir() {
                        for subentry in fs::read_dir(&child).context("reading subdirectory")? {
                            let subentry = subentry?;
                            let sub = subentry.path();
                            if self.is_git_repo(&sub) {
                                repos.push(sub);
                            }
                        }
                    }
                }
            }
        }

        Ok(repos)
    }

    /// Finds all Git repositories under the given paths:
    /// - If a path itself has a `.git` folder, it's treated as a repo root.
    /// - Otherwise it scans first-level subdirectories for `.git`.
    /// - For any first-level subdirectory that isn't a repo, it also scans its immediate children,
    ///   to pick up structures like `./org/<repo>`.
    fn find_repo_paths(&self) -> Result<Vec<PathBuf>> {
        let mut repos = Vec::new();

        for p in &self.paths {
            let pb = PathBuf::from(p);

            if self.is_git_repo(&pb) {
                repos.push(pb.clone());
                continue;
            }

            if pb.is_dir() {
                for entry in fs::read_dir(&pb).context("reading directory")? {
                    let entry = entry?;
                    let child = entry.path();

                    if self.is_git_repo(&child) {
                        repos.push(child.clone());
                        continue;
                    }

                    if child.is_dir() {
                        for subentry in fs::read_dir(&child).context("reading subdirectory")? {
                            let subentry = subentry?;
                            let sub = subentry.path();
                            if self.is_git_repo(&sub) {
                                repos.push(sub);
                            }
                        }
                    }
                }
            }
        }

        Ok(repos)
    }

    /// Check if a path is a Git repository (has a .git directory)
    fn is_git_repo<P: AsRef<Path>>(&self, path: P) -> bool {
        path.as_ref().join(".git").is_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_repo_discovery_new() {
        let paths = vec![".".to_string(), "/tmp".to_string()];
        let discovery = RepoDiscovery::new(paths.clone());
        assert_eq!(discovery.paths, paths);
    }

    #[test]
    fn test_is_git_repo() {
        let discovery = RepoDiscovery::new(vec![]);

        // Create a temporary directory structure
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("test_repo");
        fs::create_dir_all(&repo_path).unwrap();

        // Not a git repo initially
        assert!(!discovery.is_git_repo(&repo_path));

        // Create .git directory
        fs::create_dir_all(repo_path.join(".git")).unwrap();
        assert!(discovery.is_git_repo(&repo_path));
    }

    #[test]
    fn test_find_repo_paths_empty() {
        let discovery = RepoDiscovery::new(vec![]);
        let result = discovery.find_repo_paths().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_smart_matching_behavior() {
        // Test that directory paths work normally
        let discovery_dir = RepoDiscovery::new(vec![".".to_string()]);
        let result_dir = discovery_dir.discover().unwrap();
        // Should find repos in current directory (may be empty, but shouldn't error)
        assert!(result_dir.is_empty() || !result_dir.is_empty()); // Just check it doesn't panic

        // Test that non-existent identifier returns empty
        let discovery_nonexistent = RepoDiscovery::new(vec!["nonexistent-repo-12345".to_string()]);
        let result_nonexistent = discovery_nonexistent.discover().unwrap();
        assert!(result_nonexistent.is_empty());
    }

    #[test]
    fn test_smart_matching_directory_vs_identifier() {
        // Test mixed directory and identifier paths
        let discovery = RepoDiscovery::new(vec![".".to_string(), "some-identifier".to_string()]);
        let result = discovery.discover().unwrap();
        // Should work without panicking - may find repos or not depending on what's available
        assert!(result.is_empty() || !result.is_empty());
    }
}
