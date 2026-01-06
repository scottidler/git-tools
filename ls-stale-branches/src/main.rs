use chrono::{NaiveDate, Utc};
use clap::Parser;
use common::parallel::ParallelExecutor;
use common::repo::RepoDiscovery;
use eyre::{Context, Result};
use log::debug;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::process::Command;

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "stale-branches", about = "Generate a YAML report of stale branches.")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
struct Cli {
    #[arg(help = "Number of days to consider a branch stale.")]
    days: i64,

    #[arg(long, help = "Git reference to check.", default_value = "refs/remotes/origin")]
    ref_: String,

    /// Show detailed output (full YAML-style listing)
    #[arg(short = 'd', long = "detailed")]
    detailed: bool,

    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

#[derive(Serialize, Debug)]
struct AuthorBranches {
    branches: Vec<HashMap<String, i64>>,
    count: usize,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    // Discover repositories
    let discovery = RepoDiscovery::new(args.paths);
    let repos = discovery.discover().context("failed to scan for repositories")?;

    // Process each repository in parallel
    let executor = ParallelExecutor::new(repos);
    #[allow(clippy::type_complexity)]
    let repo_detailed_data: Vec<(String, Vec<(String, i64, String)>)> = executor.execute(|repo_info| {
        debug!("Processing repo: {} ({})", repo_info.slug, repo_info.path.display());

        // Query stale branches for this repository
        match get_stale_branches_for_repo(args.days, &args.ref_, &repo_info.path) {
            Ok(branch_list) => {
                if !branch_list.is_empty() {
                    Ok(Some((repo_info.slug.clone(), branch_list)))
                } else {
                    Ok(None)
                }
            }
            Err(e) => Err(e),
        }
    });

    if args.detailed {
        generate_full_yaml(&repo_detailed_data)?;
    } else {
        print_hierarchical_summary(&repo_detailed_data);
    }

    Ok(())
}

fn get_stale_branches_for_repo(
    days: i64,
    ref_: &str,
    repo_path: &std::path::Path,
) -> Result<Vec<(String, i64, String)>> {
    // First, fetch and prune branches for this repository
    Command::new("git")
        .args(["fetch", "origin", "--prune"])
        .current_dir(repo_path)
        .output()
        .wrap_err("Failed to prune local cache of git branches")?;

    let output = Command::new("git")
        .args([
            "for-each-ref",
            "--sort=-committerdate",
            ref_,
            "--format=%(committerdate:short) %(refname:short) %(committername)",
        ])
        .current_dir(repo_path)
        .output()
        .wrap_err("Failed to execute git command")?;

    let current_time = Utc::now().timestamp();
    debug!("current_time: {}", current_time);
    let result = String::from_utf8(output.stdout)?;

    let branches: Vec<(String, i64, String)> = result
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                return None;
            }
            let date_str = parts[0];
            let branch = parts[1].trim_start_matches("origin/").to_string();
            let author = parts[2..].join(" ");
            let commit_time = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)?
                .and_utc()
                .timestamp();
            let days_since_commit = (current_time - commit_time) / 86_400;

            if days_since_commit >= days {
                Some((branch, days_since_commit, author))
            } else {
                None
            }
        })
        .collect();

    Ok(branches)
}

/// Print hierarchical summary: repo -> user (count, max)
#[allow(clippy::type_complexity)]
fn print_hierarchical_summary(repo_data: &[(String, Vec<(String, i64, String)>)]) {
    for (repo_slug, branch_list) in repo_data {
        println!("{}:", repo_slug);

        // Group branches by author and calculate count/max for each
        let mut author_stats: HashMap<String, (usize, i64)> = HashMap::new();

        for (_, days, author) in branch_list {
            let entry = author_stats.entry(author.clone()).or_insert((0, 0));
            entry.0 += 1; // count
            entry.1 = entry.1.max(*days); // max age
        }

        // Sort authors by max age (descending) for consistent output
        let mut sorted_authors: Vec<_> = author_stats.iter().collect();
        sorted_authors.sort_by(|a, b| b.1 .1.cmp(&a.1 .1));

        for (author, (count, max_age)) in sorted_authors {
            println!("  {}: ({}, {})", author, count, max_age);
        }
        println!(); // Empty line between repos
    }
}

/// Generate full YAML with individual branches (detailed output)
#[allow(clippy::type_complexity)]
fn generate_full_yaml(repo_data: &[(String, Vec<(String, i64, String)>)]) -> Result<()> {
    let mut repo_dict: HashMap<String, HashMap<String, AuthorBranches>> = HashMap::new();

    for (repo_slug, branch_list) in repo_data {
        // Group branches by author first
        let mut author_branches: HashMap<String, Vec<(String, i64)>> = HashMap::new();

        for (branch, days, author) in branch_list {
            author_branches
                .entry(author.clone())
                .or_default()
                .push((branch.clone(), *days));
        }

        // Now create the authors_dict with sorted branches
        let mut authors_dict: HashMap<String, AuthorBranches> = HashMap::new();

        for (author, mut branches) in author_branches {
            // Sort branches by days (descending - oldest first)
            branches.sort_by(|a, b| b.1.cmp(&a.1));

            let branch_maps: Vec<HashMap<String, i64>> = branches
                .into_iter()
                .map(|(branch, days)| HashMap::from([(branch, days)]))
                .collect();

            let count = branch_maps.len();
            authors_dict.insert(
                author,
                AuthorBranches {
                    branches: branch_maps,
                    count,
                },
            );
        }

        repo_dict.insert(repo_slug.clone(), authors_dict);
    }

    let yaml_data = serde_yaml::to_string(&repo_dict).wrap_err("Failed to serialize data to YAML")?;
    io::stdout()
        .write_all(yaml_data.as_bytes())
        .wrap_err("Failed to write YAML to stdout")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_cli_parsing_with_paths() {
        let cli = Cli::parse_from(["ls-stale-branches", "30", "path1", "path2"]);
        assert_eq!(cli.days, 30);
        assert_eq!(cli.paths, vec!["path1", "path2"]);
        assert!(!cli.detailed);
        assert_eq!(cli.ref_, "refs/remotes/origin");
    }

    #[test]
    fn test_cli_parsing_with_detailed_flag() {
        // Test with detailed flag
        let cli = Cli::parse_from(["ls-stale-branches", "-d", "45"]);
        assert_eq!(cli.days, 45);
        assert!(cli.detailed);

        // Test default (no detailed flag)
        let cli = Cli::parse_from(["ls-stale-branches", "45"]);
        assert!(!cli.detailed);
    }

    #[test]
    fn test_author_branches_structure() {
        let branches = AuthorBranches {
            branches: vec![
                [("feature-branch".to_string(), 10)].iter().cloned().collect(),
                [("bugfix-branch".to_string(), 20)].iter().cloned().collect(),
            ],
            count: 2,
        };

        assert_eq!(branches.count, 2);
        assert_eq!(branches.branches.len(), 2);
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn test_print_hierarchical_summary_empty() {
        let repo_data: Vec<(String, Vec<(String, i64, String)>)> = vec![];

        // Should not panic with empty data
        print_hierarchical_summary(&repo_data);
    }

    #[test]
    fn test_print_hierarchical_summary_with_data() {
        let repo_data = vec![(
            "test/repo1".to_string(),
            vec![
                ("feature-branch".to_string(), 10, "user1".to_string()),
                ("bugfix-branch".to_string(), 15, "user2".to_string()),
            ],
        )];

        // Should not panic with valid data
        print_hierarchical_summary(&repo_data);
    }

    #[test]
    fn test_max_age_calculation() {
        // Test data that mimics get_stale_branches_for_repo output: (branch, age, author)
        let branch_list = [
            ("branch1".to_string(), 10, "user1".to_string()),
            ("branch2".to_string(), 25, "user2".to_string()),
            ("branch3".to_string(), 15, "user1".to_string()),
        ];

        // Test max age calculation
        let max_age = branch_list.iter().map(|(_, age, _)| *age).max().unwrap_or(0);
        assert_eq!(max_age, 25);

        // Test count
        assert_eq!(branch_list.len(), 3);
    }

    #[test]
    fn test_parallel_executor_integration() {
        // Test that ParallelExecutor works with RepoInfo
        use common::repo::RepoInfo;

        let repos = vec![
            RepoInfo::new(PathBuf::from("/test1"), "owner/repo1".to_string()),
            RepoInfo::new(PathBuf::from("/test2"), "owner/repo2".to_string()),
        ];

        let executor = ParallelExecutor::new(repos);
        let results: Vec<String> = executor.execute(|repo_info| {
            // Simple test function that returns the repo slug
            Ok(Some(repo_info.slug.clone()))
        });

        assert_eq!(results.len(), 2);
        assert!(results.contains(&"owner/repo1".to_string()));
        assert!(results.contains(&"owner/repo2".to_string()));
    }

    #[test]
    fn test_repo_discovery_integration() {
        // Test that RepoDiscovery integration works
        let discovery = RepoDiscovery::new(vec![".".to_string()]);
        let result = discovery.discover();
        assert!(result.is_ok());
    }

    #[test]
    fn test_generate_full_yaml_with_data() {
        let repo_data = vec![(
            "test/repo1".to_string(),
            vec![
                ("feature-branch".to_string(), 10, "user1".to_string()),
                ("bugfix-branch".to_string(), 15, "user2".to_string()),
            ],
        )];

        let result = generate_full_yaml(&repo_data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_branch_sorting_by_days() {
        use std::collections::HashMap;

        // Test data with multiple branches per author, unsorted
        let repo_data = vec![(
            "test/repo1".to_string(),
            vec![
                ("old-branch".to_string(), 30, "user1".to_string()),
                ("newer-branch".to_string(), 10, "user1".to_string()),
                ("oldest-branch".to_string(), 50, "user1".to_string()),
                ("middle-branch".to_string(), 20, "user1".to_string()),
                ("single-branch".to_string(), 15, "user2".to_string()),
            ],
        )];

        // Manually run the sorting logic to test it
        let mut repo_dict: HashMap<String, HashMap<String, AuthorBranches>> = HashMap::new();

        for (repo_slug, branch_list) in &repo_data {
            // Group branches by author first
            let mut author_branches: HashMap<String, Vec<(String, i64)>> = HashMap::new();

            for (branch, days, author) in branch_list {
                author_branches
                    .entry(author.clone())
                    .or_default()
                    .push((branch.clone(), *days));
            }

            // Now create the authors_dict with sorted branches
            let mut authors_dict: HashMap<String, AuthorBranches> = HashMap::new();

            for (author, mut branches) in author_branches {
                // Sort branches by days (descending - oldest first)
                branches.sort_by(|a, b| b.1.cmp(&a.1));

                let branch_maps: Vec<HashMap<String, i64>> = branches
                    .into_iter()
                    .map(|(branch, days)| HashMap::from([(branch, days)]))
                    .collect();

                let count = branch_maps.len();
                authors_dict.insert(
                    author,
                    AuthorBranches {
                        branches: branch_maps,
                        count,
                    },
                );
            }

            repo_dict.insert(repo_slug.clone(), authors_dict);
        }

        // Verify user1's branches are sorted correctly (descending by days)
        let user1_branches = &repo_dict["test/repo1"]["user1"].branches;
        assert_eq!(user1_branches.len(), 4);

        // Extract the days values to verify sorting
        let days: Vec<i64> = user1_branches
            .iter()
            .map(|branch_map| *branch_map.values().next().unwrap())
            .collect();

        // Should be sorted: [50, 30, 20, 10] (oldest first)
        assert_eq!(days, vec![50, 30, 20, 10]);

        // Verify the branch names are in the correct order
        let branch_names: Vec<String> = user1_branches
            .iter()
            .map(|branch_map| branch_map.keys().next().unwrap().clone())
            .collect();

        assert_eq!(
            branch_names,
            vec![
                "oldest-branch".to_string(),
                "old-branch".to_string(),
                "middle-branch".to_string(),
                "newer-branch".to_string()
            ]
        );

        // Verify user2 has single branch
        let user2_branches = &repo_dict["test/repo1"]["user2"].branches;
        assert_eq!(user2_branches.len(), 1);
        assert_eq!(*user2_branches[0].values().next().unwrap(), 15);
    }
}
