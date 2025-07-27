use std::sync::Mutex;
use eyre::Result;
use rayon::prelude::*;
use super::repo::RepoInfo;

/// A framework for executing work on repositories in parallel
pub struct ParallelExecutor {
    repos: Vec<RepoInfo>,
}

impl ParallelExecutor {
    /// Create a new parallel executor with discovered repositories
    pub fn new(repos: Vec<RepoInfo>) -> Self {
        Self { repos }
    }

    /// Execute a function on each repository in parallel, collecting successful results
    /// The function should return Ok(Some(T)) for results to include, Ok(None) to skip, or Err for failures
    pub fn execute<T, F>(&self, work_fn: F) -> Vec<T>
    where
        T: Send,
        F: Fn(&RepoInfo) -> Result<Option<T>> + Sync,
    {
        self.repos
            .par_iter()
            .filter_map(|repo_info| {
                match work_fn(repo_info) {
                    Ok(Some(result)) => Some(result),
                    Ok(None) => None,
                    Err(e) => {
                        eprintln!("❌ {}: {}", repo_info.slug, e);
                        None
                    }
                }
            })
            .collect()
    }

    /// Execute a function on each repository in parallel, collecting all results (including errors)
    /// Returns a Vec of Results, preserving both successes and failures
    pub fn execute_all<T, F>(&self, work_fn: F) -> Vec<Result<T>>
    where
        T: Send,
        F: Fn(&RepoInfo) -> Result<T> + Sync,
    {
        self.repos
            .par_iter()
            .map(|repo_info| work_fn(repo_info))
            .collect()
    }

    /// Execute a function on each repository in parallel, with mutable shared state
    /// Useful when you need to accumulate results or maintain shared state across parallel execution
    pub fn execute_with_state<T, S, F>(&self, shared_state: S, work_fn: F) -> S
    where
        S: Send,
        F: Fn(&RepoInfo, &Mutex<S>) -> Result<Option<T>> + Sync,
        T: Send,
    {
        let state_mutex = Mutex::new(shared_state);

        self.repos
            .par_iter()
            .for_each(|repo_info| {
                match work_fn(repo_info, &state_mutex) {
                    Ok(_) => {},
                    Err(e) => {
                        eprintln!("❌ {}: {}", repo_info.slug, e);
                    }
                }
            });

        state_mutex.into_inner().unwrap()
    }

    /// Get the list of repositories being processed
    pub fn repos(&self) -> &[RepoInfo] {
        &self.repos
    }

    /// Get the number of repositories being processed
    pub fn len(&self) -> usize {
        self.repos.len()
    }

    /// Check if there are no repositories to process
    pub fn is_empty(&self) -> bool {
        self.repos.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_parallel_executor_new() {
        let repos = vec![
            RepoInfo::new(PathBuf::from("/test1"), "owner/repo1".to_string()),
            RepoInfo::new(PathBuf::from("/test2"), "owner/repo2".to_string()),
        ];
        let executor = ParallelExecutor::new(repos.clone());
        assert_eq!(executor.len(), 2);
        assert_eq!(executor.repos(), &repos);
    }

    #[test]
    fn test_execute_success() {
        let repos = vec![
            RepoInfo::new(PathBuf::from("/test1"), "owner/repo1".to_string()),
            RepoInfo::new(PathBuf::from("/test2"), "owner/repo2".to_string()),
        ];
        let executor = ParallelExecutor::new(repos);

        let results = executor.execute(|repo| {
            Ok(Some(repo.slug.clone()))
        });

        assert_eq!(results.len(), 2);
        assert!(results.contains(&"owner/repo1".to_string()));
        assert!(results.contains(&"owner/repo2".to_string()));
    }

    #[test]
    fn test_execute_with_filtering() {
        let repos = vec![
            RepoInfo::new(PathBuf::from("/test1"), "owner/repo1".to_string()),
            RepoInfo::new(PathBuf::from("/test2"), "owner/repo2".to_string()),
        ];
        let executor = ParallelExecutor::new(repos);

        let results = executor.execute(|repo| {
            if repo.slug.contains("repo1") {
                Ok(Some(repo.slug.clone()))
            } else {
                Ok(None) // Skip repo2
            }
        });

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "owner/repo1");
    }

    #[test]
    fn test_execute_with_state() {
        let repos = vec![
            RepoInfo::new(PathBuf::from("/test1"), "owner/repo1".to_string()),
            RepoInfo::new(PathBuf::from("/test2"), "owner/repo2".to_string()),
        ];
        let executor = ParallelExecutor::new(repos);

        let final_state = executor.execute_with_state(Vec::<String>::new(), |repo, state| {
            let mut state = state.lock().unwrap();
            state.push(repo.slug.clone());
            Ok(Some(()))
        });

        assert_eq!(final_state.len(), 2);
        assert!(final_state.contains(&"owner/repo1".to_string()));
        assert!(final_state.contains(&"owner/repo2".to_string()));
    }
}
