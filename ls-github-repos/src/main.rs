use clap::{Parser, ValueEnum};
use reqwest::{Client, header};
use serde_json::Value;
use tokio;
use eyre::{Result, eyre};
use std::{fs, fmt};
use std::path::PathBuf;
use shellexpand;
use log::debug;
use env_logger;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// Supply the GitHub organization or user name
    #[clap(value_parser)]
    name: String,

    /// Path to the directory containing the GitHub tokens
    #[clap(short, long, default_value = "~/.config/github/tokens")]
    token_path: String,

    /// The type of repository owner, either 'user' or 'org'
    #[clap(short, long, value_enum, default_value = "org")]
    repo_type: RepoType,

    /// Include archived repositories
    #[clap(short, long, action = clap::ArgAction::SetTrue)]
    archived: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum RepoType {
    /// User type repository
    User,
    /// Organization type repository
    Org,
}

impl fmt::Display for RepoType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", match self {
            RepoType::User => "users",
            RepoType::Org => "orgs",
        })
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    let expanded_token_path = shellexpand::tilde(&args.token_path).to_string();
    let token_path = PathBuf::from(expanded_token_path);
    let token_file_path = token_path.join(&args.name);

    let token = fs::read_to_string(token_file_path)
        .map_err(|e| eyre!("Failed to read token file: {}", e))?
        .trim().to_string();

    debug!("Trimmed token: '{}'", token);

    let repo_names = ls_github_repos(args.repo_type, &args.name, args.archived, &token).await?;
    for repo_name in repo_names {
        println!("{}", repo_name);
    }
    Ok(())
}

async fn ls_github_repos(repo_type: RepoType, name: &str, archived: bool, token: &str) -> Result<Vec<String>> {
    let client = Client::new();
    let base_url = format!("https://api.github.com/{}/{}", repo_type, name);
    let url = format!("{}/repos", base_url);
    let mut headers = header::HeaderMap::new();

    debug!("Setting headers with token: '{}'", token);
    let auth_value = format!("token {}", token);
    headers.insert("Authorization", header::HeaderValue::from_str(&auth_value)
        .map_err(|e| eyre!("Failed to parse 'Authorization' header value: {}", e))?);
    headers.insert("User-Agent", header::HeaderValue::from_static("reqwest"));
    headers.insert("Accept", header::HeaderValue::from_static("application/vnd.github.v3+json"));

    debug!("Headers set successfully: {:?}", headers);

    let mut repo_names = Vec::new();
    let mut page = 1;

    loop {
        let response = client.get(&url)
            .headers(headers.clone())
            .query(&[("page", page.to_string()), ("per_page", "100".to_string())])
            .send()
            .await?
            .json::<Vec<Value>>()
            .await?;

        if response.is_empty() {
            break;
        }

        for repo in response {
            if archived || !repo["archived"].as_bool().unwrap_or(false) {
                if let Some(repo_name) = repo["full_name"].as_str() {
                    repo_names.push(repo_name.to_owned());
                }
            }
        }
        page += 1;
    }

    repo_names.sort_unstable();
    Ok(repo_names)
}
