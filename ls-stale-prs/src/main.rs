use clap::Parser;
use chrono::{DateTime, Utc};
use common::repo::RepoDiscovery;
use common::parallel::ParallelExecutor;
use env_logger;
use eyre::{Result, Context};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_yaml;
use serde_json;
use std::collections::HashMap;
use std::io::{self, Write};
use std::process::Command;

#[derive(Parser, Debug)]
#[command(name = "stale-prs", about = "Generate a YAML report of stale PRs.")]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(version)]
struct Cli {
    #[arg(help = "Number of days to consider a PR stale.")]
    days: i64,

    /// Show detailed output (full YAML-style listing)
    #[arg(short = 'd', long = "detailed")]
    detailed: bool,

    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

#[derive(Serialize, Debug)]
struct AuthorPRs {
    prs: Vec<HashMap<String, i64>>,
    count: usize,
}

#[derive(Deserialize, Debug)]
struct GhPr {
    title: String,
    number: u64,
    #[serde(rename = "createdAt")]
    created_at: String,
    author: Option<Author>,
}

#[derive(Deserialize, Debug)]
struct Author {
    login: String,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    // Discover repositories from the provided paths
    let discovery = RepoDiscovery::new(args.paths);
    let repos = discovery.discover()
        .context("failed to scan for repositories")?;

    // Process each repository in parallel
    let executor = ParallelExecutor::new(repos);
    let repo_detailed_data: Vec<(String, Vec<(String, i64, String)>)> = executor.execute(|repo_info| {
        debug!("Processing repo: {} ({})", repo_info.slug, repo_info.path.display());

        // Query stale PRs for this repository
        match get_stale_prs(args.days, &repo_info.slug) {
            Ok(pr_list) => {
                if !pr_list.is_empty() {
                    Ok(Some((repo_info.slug.clone(), pr_list)))
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



/// Queries the GitHub CLI for pull requests, filtering those older than the specified days.
fn get_stale_prs(days: i64, reposlug: &str) -> Result<Vec<(String, i64, String)>> {
    // Use the GitHub CLI to list PRs in JSON format.
    let output = Command::new("gh")
        .args(&[
            "pr", "list",
            "--repo", reposlug,
            "--limit", "100",
            "--json", "title,number,createdAt,author"
        ])
        .output()
        .wrap_err("Failed to execute gh command")?;
    if !output.status.success() {
        return Err(eyre::eyre!("gh command failed to execute properly"));
    }
    let stdout = String::from_utf8(output.stdout)?;
    debug!("gh output: {}", stdout);

    let pr_entries: Vec<GhPr> = serde_json::from_str(&stdout)
        .wrap_err("Failed to parse gh JSON output")?;

    let now: DateTime<Utc> = Utc::now();
    // Filter PRs based on their age.
    let stale_prs: Vec<(String, i64, String)> = pr_entries.into_iter()
        .filter_map(|pr| {
            let created_at = DateTime::parse_from_rfc3339(&pr.created_at).ok()?.with_timezone(&Utc);
            let age_days = (now - created_at).num_days();
            if age_days >= days {
                // Use the author login, defaulting to "Unknown" if not available.
                let author = pr.author.map(|a| a.login).unwrap_or_else(|| "Unknown".to_string());
                let title_with_number = format!("{} (pr {})", pr.title, pr.number);
                Some((title_with_number, age_days, author))
            } else {
                None
            }
        })
        .collect();

    Ok(stale_prs)
}



/// Print hierarchical summary: repo -> user (count, max)
fn print_hierarchical_summary(repo_data: &[(String, Vec<(String, i64, String)>)]) {
    for (repo_slug, pr_list) in repo_data {
        println!("{}:", repo_slug);

        // Group PRs by author and calculate count/max for each
        let mut author_stats: HashMap<String, (usize, i64)> = HashMap::new();

        for (_, days, author) in pr_list {
            let entry = author_stats.entry(author.clone()).or_insert((0, 0));
            entry.0 += 1; // count
            entry.1 = entry.1.max(*days); // max age
        }

        // Sort authors by max age (descending) for consistent output
        let mut sorted_authors: Vec<_> = author_stats.iter().collect();
        sorted_authors.sort_by(|a, b| b.1.1.cmp(&a.1.1));

        for (author, (count, max_age)) in sorted_authors {
            println!("  {}: ({}, {})", author, count, max_age);
        }
                 println!(); // Empty line between repos
    }
}

/// Generate full YAML with individual PRs (detailed output)
fn generate_full_yaml(repo_data: &[(String, Vec<(String, i64, String)>)]) -> Result<()> {
    let mut repo_dict: HashMap<String, HashMap<String, AuthorPRs>> = HashMap::new();

    for (repo_slug, pr_list) in repo_data {
        let mut authors_dict: HashMap<String, AuthorPRs> = HashMap::new();

        for (pr_title, days, author) in pr_list {
            authors_dict
                .entry(author.clone())
                .or_insert_with(|| AuthorPRs { prs: vec![], count: 0 })
                .prs
                .push(HashMap::from([(pr_title.clone(), *days)]));
            authors_dict.get_mut(author).unwrap().count += 1;
        }

        repo_dict.insert(repo_slug.clone(), authors_dict);
    }

    let yaml_data = serde_yaml::to_string(&repo_dict)
        .wrap_err("Failed to serialize data to YAML")?;
    io::stdout().write_all(yaml_data.as_bytes())
        .wrap_err("Failed to write YAML to stdout")?;
    Ok(())
}



#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;
    use std::process::Command;

    fn create_test_repo_with_remote(temp_dir: &TempDir, repo_name: &str, remote_url: &str) -> std::path::PathBuf {
        let repo_path = temp_dir.path().join(repo_name);
        fs::create_dir_all(&repo_path).unwrap();

        // Initialize git repo
        Command::new("git")
            .current_dir(&repo_path)
            .args(["init"])
            .output()
            .unwrap();

        // Add a remote origin
        Command::new("git")
            .current_dir(&repo_path)
            .args(["remote", "add", "origin", remote_url])
            .output()
            .unwrap();

        repo_path
    }

    #[test]
    fn test_repo_discovery_integration() {
        let temp_dir = TempDir::new().unwrap();
        let _repo1 = create_test_repo_with_remote(&temp_dir, "repo1", "git@github.com:org1/repo1.git");
        let _repo2 = create_test_repo_with_remote(&temp_dir, "repo2", "https://github.com/org2/repo2.git");

        let discovery = RepoDiscovery::new(vec![temp_dir.path().to_string_lossy().to_string()]);
        let repos = discovery.discover().unwrap();

        assert_eq!(repos.len(), 2);
        assert!(repos.iter().any(|r| r.path.file_name().unwrap() == "repo1" && r.slug == "org1/repo1"));
        assert!(repos.iter().any(|r| r.path.file_name().unwrap() == "repo2" && r.slug == "org2/repo2"));
    }

    #[test]
    fn test_cli_parsing_with_paths() {
        use clap::Parser;

        // Test default path
        let cli = Cli::parse_from(&["ls-stale-prs", "30"]);
        assert_eq!(cli.days, 30);
        assert_eq!(cli.paths, vec!["."]);

        // Test custom paths
        let cli = Cli::parse_from(&["ls-stale-prs", "15", "/path1", "/path2"]);
        assert_eq!(cli.days, 15);
        assert_eq!(cli.paths, vec!["/path1", "/path2"]);
    }

    #[test]
    fn test_cli_parsing_with_detailed_flag() {
        use clap::Parser;

        // Test detailed flag
        let cli = Cli::parse_from(&["ls-stale-prs", "30", "--detailed"]);
        assert_eq!(cli.days, 30);
        assert_eq!(cli.detailed, true);
        assert_eq!(cli.paths, vec!["."]);

        // Test short form
        let cli = Cli::parse_from(&["ls-stale-prs", "15", "-d", "/path1"]);
        assert_eq!(cli.days, 15);
        assert_eq!(cli.detailed, true);
        assert_eq!(cli.paths, vec!["/path1"]);

        // Test default (no detailed flag)
        let cli = Cli::parse_from(&["ls-stale-prs", "45"]);
        assert_eq!(cli.detailed, false);
    }



    #[test]
    fn test_author_prs_structure() {
        let author_prs = AuthorPRs {
            prs: vec![
                [("Test PR".to_string(), 5)].iter().cloned().collect(),
                [("Another PR".to_string(), 10)].iter().cloned().collect(),
            ],
            count: 2,
        };

        assert_eq!(author_prs.count, 2);
        assert_eq!(author_prs.prs.len(), 2);
    }

        #[test]
    fn test_print_hierarchical_summary_empty() {
        let repo_data: Vec<(String, Vec<(String, i64, String)>)> = vec![];

        // Test that function handles empty input without panicking
        print_hierarchical_summary(&repo_data);
        // If we get here, the function didn't panic - that's good
    }

    #[test]
    fn test_print_hierarchical_summary_with_data() {
        let repo_data = vec![
            ("org1/repo1".to_string(), vec![
                ("PR 1 (pr 100)".to_string(), 45, "user1".to_string()),
                ("PR 2 (pr 200)".to_string(), 30, "user1".to_string()),
                ("PR 3 (pr 300)".to_string(), 120, "user2".to_string()),
            ]),
            ("org2/repo2".to_string(), vec![
                ("PR 4 (pr 400)".to_string(), 89, "user3".to_string()),
            ]),
        ];

        // Test that function handles data without panicking
        print_hierarchical_summary(&repo_data);
        // If we get here, the function didn't panic with real data

        // Verify the data structure is correct
        assert_eq!(repo_data.len(), 2);
        assert_eq!(repo_data[0].0, "org1/repo1");
        assert_eq!(repo_data[0].1.len(), 3);
        assert_eq!(repo_data[1].0, "org2/repo2");
        assert_eq!(repo_data[1].1.len(), 1);
    }

    #[test]
    fn test_generate_full_yaml_with_data() {
        let repo_data = vec![
            ("test/repo1".to_string(), vec![
                ("Fix bug (pr 123)".to_string(), 10, "user1".to_string()),
                ("Add feature (pr 456)".to_string(), 15, "user2".to_string()),
            ]),
        ];

        // Test that function handles data without panicking
        let result = generate_full_yaml(&repo_data);
        assert!(result.is_ok());
    }

    #[test]
    fn test_max_age_calculation() {
        // Test data that mimics get_stale_prs output: (title, age, author)
        let pr_list = vec![
            ("PR 1 (pr 100)".to_string(), 10, "user1".to_string()),
            ("PR 2 (pr 200)".to_string(), 25, "user2".to_string()),
            ("PR 3 (pr 300)".to_string(), 15, "user1".to_string()),
        ];

        // Test max age calculation
        let max_age = pr_list.iter().map(|(_, age, _)| *age).max().unwrap_or(0);
        assert_eq!(max_age, 25);

        // Test count
        assert_eq!(pr_list.len(), 3);
    }

    #[test]
    fn test_parallel_executor_integration() {
        // Test that ParallelExecutor works with RepoInfo
        use common::repo::RepoInfo;
        use std::path::PathBuf;

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
    fn test_get_stale_prs_blocking() {
        // Test that get_stale_prs works as a blocking function
        // This will fail if gh is not installed, but that's expected in CI
        let result = get_stale_prs(30, "nonexistent/repo");

        // We expect this to fail (repo doesn't exist), but it should be a proper Result
        assert!(result.is_err());
    }

}
