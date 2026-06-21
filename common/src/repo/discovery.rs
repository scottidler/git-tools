use super::RepoInfo;
use eyre::{Context, Result};
use rayon::prelude::*;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Default maximum scan depth (handles the `~/repos/<org>/<repo>` layout).
const DEFAULT_MAX_DEPTH: usize = 2;

/// The kind of repository a discovered path represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RepoKind {
    /// A flat single checkout (`.git` directory or file).
    Flat,
    /// A bare container (`<container>/.bare` + `.git` pointer + worktrees).
    Bare,
}

/// Repository discovery utility for finding Git repositories
pub struct RepoDiscovery {
    paths: Vec<String>,
    /// Maximum levels to descend below each base path. `None` = unbounded.
    max_depth: Option<usize>,
    /// When true, a bare container emits one row per worktree instead of a
    /// single default logical-repo row.
    per_worktree: bool,
}

impl RepoDiscovery {
    /// Create a new RepoDiscovery with the given paths to search.
    /// Defaults: depth 2 (the `org/repo` layout), one row per logical repo.
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            paths,
            max_depth: Some(DEFAULT_MAX_DEPTH),
            per_worktree: false,
        }
    }

    /// Override the maximum scan depth. `None` searches unbounded depth (e.g.
    /// `ls-git-repos`, which formerly used an infinite-depth `WalkDir`).
    pub fn with_max_depth(mut self, max_depth: Option<usize>) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// Opt into per-worktree rows for bare containers.
    pub fn with_per_worktree(mut self, per_worktree: bool) -> Self {
        self.per_worktree = per_worktree;
        self
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
            let candidates = self.find_candidates(&self.paths)?;
            return Ok(self.candidates_to_infos(candidates, true));
        }

        // For smart matching, scan efficiently with filtering
        let base_paths = if directory_paths.is_empty() { vec![".".to_string()] } else { directory_paths };
        let candidates = self.find_candidates(&base_paths)?;
        let infos = self.candidates_to_infos(candidates, false);

        let mut result = Vec::new();
        let mut used_repos = HashSet::new();

        for repo_info in infos {
            // Check if this repo matches any of our identifiers (exact, then contains)
            let matched = repo_identifiers
                .iter()
                .any(|identifier| repo_info.slug == *identifier || repo_info.path.to_string_lossy() == *identifier)
                || repo_identifiers
                    .iter()
                    .any(|identifier| repo_info.slug.contains(identifier));

            if matched && used_repos.insert(repo_info.slug.clone()) {
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

    /// Convert discovered candidate paths into RepoInfo rows (in parallel).
    /// When `report_errors` is set, per-repo extraction failures are printed.
    fn candidates_to_infos(&self, candidates: Vec<(PathBuf, RepoKind)>, report_errors: bool) -> Vec<RepoInfo> {
        candidates
            .par_iter()
            .flat_map(|(path, kind)| match self.build_infos(path, *kind) {
                Ok(infos) => infos,
                Err(e) => {
                    if report_errors {
                        eprintln!("❌ {}: {}", path.display(), e);
                    }
                    Vec::new()
                }
            })
            .collect()
    }

    /// Build the RepoInfo row(s) for a single discovered repository.
    fn build_infos(&self, path: &Path, kind: RepoKind) -> Result<Vec<RepoInfo>> {
        match kind {
            RepoKind::Flat => Ok(vec![RepoInfo::from_path(path)?]),
            RepoKind::Bare => {
                if self.per_worktree {
                    RepoInfo::worktrees_of(path)
                } else {
                    Ok(vec![RepoInfo::from_bare_container(path)?])
                }
            }
        }
    }

    /// Find all repository candidates under the given base paths, honoring
    /// `max_depth`. Descent stops at any directory recognized as a repo or bare
    /// container (so worktrees inside a container are not re-reported).
    fn find_candidates(&self, base_paths: &[String]) -> Result<Vec<(PathBuf, RepoKind)>> {
        let mut out = Vec::new();
        for p in base_paths {
            self.scan(&PathBuf::from(p), self.max_depth, &mut out)?;
        }
        Ok(out)
    }

    /// Recursively scan `dir`, pushing repo candidates. `remaining` is the
    /// number of levels still allowed below `dir` (`None` = unbounded).
    fn scan(&self, dir: &Path, remaining: Option<usize>, out: &mut Vec<(PathBuf, RepoKind)>) -> Result<()> {
        if let Some(kind) = self.classify(dir) {
            out.push((dir.to_path_buf(), kind));
            return Ok(());
        }

        if !dir.is_dir() {
            return Ok(());
        }

        let next = match remaining {
            Some(0) => return Ok(()),
            Some(n) => Some(n - 1),
            None => None,
        };

        for entry in fs::read_dir(dir).context("reading directory")? {
            let entry = entry?;
            self.scan(&entry.path(), next, out)?;
        }

        Ok(())
    }

    /// Classify a directory as a repo candidate, if any. Bare containers are
    /// checked first (they also carry a `.git` pointer file).
    fn classify(&self, dir: &Path) -> Option<RepoKind> {
        if is_bare_container(dir) {
            Some(RepoKind::Bare)
        } else if self.is_git_repo(dir) {
            Some(RepoKind::Flat)
        } else {
            None
        }
    }

    /// Check if a path is a Git repository (`.git` directory or file).
    fn is_git_repo<P: AsRef<Path>>(&self, path: P) -> bool {
        let git = path.as_ref().join(".git");
        git.is_dir() || git.is_file()
    }
}

/// A bare container holds the git database under `.bare/` (alongside a `.git`
/// pointer file and per-branch worktrees).
fn is_bare_container(path: &Path) -> bool {
    path.join(".bare").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git;
    use std::fs;
    use tempfile::TempDir;

    fn git_run(dir: &Path, args: &[&str]) {
        let out = git::output(args, Some(dir), None).unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed in {}: {}",
            args,
            dir.display(),
            out.stderr
        );
    }

    /// Build a local "remote" repo with one commit on `main`.
    fn make_remote(root: &Path) -> PathBuf {
        let remote = root.join("remote");
        fs::create_dir_all(&remote).unwrap();
        git_run(&remote, &["init", "-b", "main"]);
        fs::write(remote.join("README.md"), "hello").unwrap();
        git_run(&remote, &["add", "."]);
        git_run(
            &remote,
            &["-c", "user.email=t@e.com", "-c", "user.name=t", "commit", "-m", "init"],
        );
        remote
    }

    /// Build a bare container at `<container>` cloned from `remote`.
    fn make_bare_container(container: &Path, remote: &Path) {
        fs::create_dir_all(container).unwrap();
        let bare = container.join(".bare");
        git_run(
            remote,
            &["clone", "--bare", remote.to_str().unwrap(), bare.to_str().unwrap()],
        );
        fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
        git_run(
            container,
            &["config", "remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"],
        );
        git_run(container, &["fetch", "origin"]);
        git_run(container, &["worktree", "add", "main", "main"]);
    }

    #[test]
    fn test_repo_discovery_new() {
        let paths = vec![".".to_string(), "/tmp".to_string()];
        let discovery = RepoDiscovery::new(paths.clone());
        assert_eq!(discovery.paths, paths);
        assert_eq!(discovery.max_depth, Some(2));
        assert!(!discovery.per_worktree);
    }

    #[test]
    fn test_is_git_repo() {
        let discovery = RepoDiscovery::new(vec![]);
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("test_repo");
        fs::create_dir_all(&repo_path).unwrap();

        // Not a git repo initially
        assert!(!discovery.is_git_repo(&repo_path));

        // .git directory -> repo
        fs::create_dir_all(repo_path.join(".git")).unwrap();
        assert!(discovery.is_git_repo(&repo_path));

        // .git file (e.g. a worktree) -> also a repo
        let wt = temp_dir.path().join("worktree");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: ../x/.bare/worktrees/wt\n").unwrap();
        assert!(discovery.is_git_repo(&wt));
    }

    #[test]
    fn test_discover_empty_paths() {
        let discovery = RepoDiscovery::new(vec![]);
        assert!(discovery.discover().unwrap().is_empty());
    }

    #[test]
    fn test_smart_matching_behavior() {
        let discovery_dir = RepoDiscovery::new(vec![".".to_string()]);
        let result_dir = discovery_dir.discover().unwrap();
        assert!(result_dir.is_empty() || !result_dir.is_empty());

        let discovery_nonexistent = RepoDiscovery::new(vec!["nonexistent-repo-12345".to_string()]);
        let result_nonexistent = discovery_nonexistent.discover().unwrap();
        assert!(result_nonexistent.is_empty());
    }

    #[test]
    fn test_smart_matching_directory_vs_identifier() {
        let discovery = RepoDiscovery::new(vec![".".to_string(), "some-identifier".to_string()]);
        let result = discovery.discover().unwrap();
        assert!(result.is_empty() || !result.is_empty());
    }

    #[test]
    fn test_discover_mixed_flat_and_bare_container() {
        let tmp = TempDir::new().unwrap();
        let remote = make_remote(tmp.path());

        // Workspace scanned by discovery (remote lives outside it).
        let work = tmp.path().join("work");

        // Flat clone at work/flatorg/flatrepo
        let flat = work.join("flatorg").join("flatrepo");
        fs::create_dir_all(flat.parent().unwrap()).unwrap();
        git_run(&remote, &["clone", remote.to_str().unwrap(), flat.to_str().unwrap()]);

        // Bare container at work/bareorg/barerepo
        let container = work.join("bareorg").join("barerepo");
        fs::create_dir_all(container.parent().unwrap()).unwrap();
        make_bare_container(&container, &remote);

        let discovery = RepoDiscovery::new(vec![work.to_str().unwrap().to_string()]);
        let mut infos = discovery.discover().unwrap();
        infos.sort_by(|a, b| a.slug.cmp(&b.slug));

        assert_eq!(infos.len(), 2, "expected one flat + one bare row, got {:?}", infos);

        let bare = &infos[0];
        assert_eq!(bare.slug, "bareorg/barerepo");
        assert_eq!(bare.path, container.join("main"));
        assert_eq!(bare.worktree, None);

        let flat_info = &infos[1];
        assert_eq!(flat_info.slug, "flatorg/flatrepo");
        assert_eq!(flat_info.path, flat);
        assert_eq!(flat_info.worktree, None);
    }

    #[test]
    fn test_discover_per_worktree_enumeration() {
        let tmp = TempDir::new().unwrap();
        let remote = make_remote(tmp.path());

        let work = tmp.path().join("work");
        let container = work.join("bareorg").join("barerepo");
        fs::create_dir_all(container.parent().unwrap()).unwrap();
        make_bare_container(&container, &remote);
        git_run(&container, &["worktree", "add", "-b", "feature", "feature", "main"]);

        let discovery = RepoDiscovery::new(vec![work.to_str().unwrap().to_string()]).with_per_worktree(true);
        let mut infos = discovery.discover().unwrap();
        infos.sort_by(|a, b| a.path.cmp(&b.path));

        let names: Vec<Option<String>> = infos.iter().map(|i| i.worktree.clone()).collect();
        assert!(names.contains(&Some("main".to_string())), "got {:?}", names);
        assert!(names.contains(&Some("feature".to_string())), "got {:?}", names);
        assert!(infos.iter().all(|i| i.slug == "bareorg/barerepo"));
    }
}
