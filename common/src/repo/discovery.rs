use std::path::{Path, PathBuf};
use std::fs;
use eyre::{Result, Context};
use super::RepoInfo;

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
    pub fn discover(&self) -> Result<Vec<RepoInfo>> {
        let repo_paths = self.find_repo_paths()?;
        let mut repos = Vec::new();
        
        for path in repo_paths {
            match RepoInfo::from_path(&path) {
                Ok(repo_info) => repos.push(repo_info),
                Err(e) => {
                    eprintln!("❌ {}: {}", path.display(), e);
                    continue;
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
} 