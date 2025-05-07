use clap::Parser;
use chrono::{DateTime, Utc};
use env_logger;
use eyre::{Result, Context};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_yaml;
use serde_json;
use std::collections::HashMap;
use std::io::{self, Write};
use tokio::process::Command;

#[derive(Parser, Debug)]
#[command(name = "stale-prs", about = "Generate a YAML report of stale PRs.")]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(version)]
struct Cli {
    #[arg(help = "Number of days to consider a PR stale.")]
    days: i64,
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

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    // Determine the repository slug from the local .git configuration.
    let reposlug = get_reposlug().await?;
    debug!("Reposlug: {}", reposlug);

    // Query stale PRs using gh.
    let pr_list = get_stale_prs(args.days, &reposlug).await?;
    generate_yaml(&pr_list)?;

    Ok(())
}

/// Retrieves the repository slug (e.g. "org/repo") by calling "git remote get-url origin".
async fn get_reposlug() -> Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .wrap_err("Failed to get origin URL using git")?;
    if !output.status.success() {
        return Err(eyre::eyre!("git command failed to execute properly"));
    }
    let url = String::from_utf8(output.stdout)?.trim().to_string();

    // Handle git remote URL in both SSH and HTTPS formats.
    let reposlug = if url.starts_with("git@") {
        // Example: git@github.com:org/repo.git
        url.split(':')
            .nth(1)
            .and_then(|s| s.strip_suffix(".git").or(Some(s)))
            .unwrap_or(&url)
            .to_string()
    } else if url.starts_with("https://") || url.starts_with("http://") {
        // Example: https://github.com/org/repo.git
        let parts: Vec<&str> = url.split('/').collect();
        if parts.len() < 2 {
            return Err(eyre::eyre!("Invalid git URL format"));
        }
        let org = parts.get(parts.len()-2).unwrap();
        let repo = parts.get(parts.len()-1).unwrap().strip_suffix(".git").unwrap_or(*parts.get(parts.len()-1).unwrap());
        format!("{}/{}", org, repo)
    } else {
        return Err(eyre::eyre!("Unknown git URL format"));
    };
    Ok(reposlug)
}

/// Queries the GitHub CLI for pull requests, filtering those older than the specified days.
async fn get_stale_prs(days: i64, reposlug: &str) -> Result<Vec<(String, i64, String)>> {
    // Use the GitHub CLI to list PRs in JSON format.
    let output = Command::new("gh")
        .args(&[
            "pr", "list",
            "--repo", reposlug,
            "--limit", "100",
            "--json", "title,number,createdAt,author"
        ])
        .output()
        .await
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

/// Serializes the PR data into YAML, grouping by PR author.
fn generate_yaml(prs: &[(String, i64, String)]) -> Result<()> {
    let mut authors_dict: HashMap<String, AuthorPRs> = HashMap::new();

    for (pr_title, days, author) in prs {
        authors_dict
            .entry(author.clone())
            .or_insert_with(|| AuthorPRs { prs: vec![], count: 0 })
            .prs
            .push(HashMap::from([(pr_title.clone(), *days)]));
        authors_dict.get_mut(author).unwrap().count += 1;
    }

    let yaml_data = serde_yaml::to_string(&authors_dict)
        .wrap_err("Failed to serialize data to YAML")?;
    io::stdout().write_all(yaml_data.as_bytes())
        .wrap_err("Failed to write YAML to stdout")?;
    Ok(())
}
