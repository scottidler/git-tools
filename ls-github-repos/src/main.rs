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

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/git_describe.rs"));
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[command(name = "ls-github-repos", about = "list all repos under an org or user")]
#[command(version = built_info::GIT_DESCRIBE)]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[clap(value_parser)]
    name: String,

    #[clap(short, long, default_value = "~/.config/github/tokens")]
    token_path: String,

    #[clap(short, long, value_enum, default_value = "org")]
    repo_type: RepoType,

    #[clap(short = 'A', long, action = clap::ArgAction::SetTrue)]
    archived: bool,

    #[clap(short = 'a', long, action = clap::ArgAction::SetTrue)]
    age: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
enum RepoType {
    User,
    Org,
}

impl RepoType {
    fn repo_url(&self, name: &str) -> String {
        match self {
            RepoType::User => format!("https://api.github.com/users/{}/repos", name),
            RepoType::Org => format!("https://api.github.com/orgs/{}/repos", name),
        }
    }
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

    let repo_type = determine_repo_type(&args.name, &token).await?;
    let repo_data = ls_github_repos(repo_type, &args.name, args.archived, &token).await?;

    for (repo_name, created_at) in repo_data {
        if args.age {
            println!("{} {}", created_at, repo_name);
        } else {
            println!("{}", repo_name);
        }
    }
    Ok(())
}

async fn determine_repo_type(name: &str, token: &str) -> Result<RepoType> {
    let client = Client::new();
    let mut headers = header::HeaderMap::new();

    let auth_value = format!("token {}", token);
    headers.insert("Authorization", header::HeaderValue::from_str(&auth_value)
        .map_err(|e| eyre!("Failed to parse 'Authorization' header value: {}", e))?);
    headers.insert("User-Agent", header::HeaderValue::from_static("reqwest"));

    let user_url = format!("https://api.github.com/users/{}", name);

    let user_response = client.get(&user_url).headers(headers.clone()).send().await?;
    if user_response.status().is_success() {
        let user_data: Value = user_response.json().await?;
        if let Some(user_type) = user_data["type"].as_str() {
            debug!("GitHub API response for '{}': {:?}", name, user_data);
            match user_type {
                "User" => {
                    debug!("'{}' is identified as a User", name);
                    return Ok(RepoType::User);
                }
                "Organization" => {
                    debug!("'{}' is identified as an Organization", name);
                    return Ok(RepoType::Org);
                }
                _ => {
                    debug!("Unknown type for '{}': {}", name, user_type);
                }
            }
        }
    }

    Err(eyre!("'{}' is neither a valid GitHub user nor organization, or your token lacks access.", name))
}

async fn ls_github_repos(repo_type: RepoType, name: &str, archived: bool, token: &str) -> Result<Vec<(String, String)>> {
    let client = Client::new();
    let url = repo_type.repo_url(name);
    let mut headers = header::HeaderMap::new();
    let auth_value = format!("token {}", token);

    headers.insert("Authorization", header::HeaderValue::from_str(&auth_value)
        .map_err(|e| eyre!("Failed to parse 'Authorization' header value: {}", e))?);
    headers.insert("User-Agent", header::HeaderValue::from_static("reqwest"));
    headers.insert("Accept", header::HeaderValue::from_static("application/vnd.github.v3+json"));

    let mut repo_data = Vec::new();
    let mut page = 1;

    loop {
        let response = client.get(&url)
            .headers(headers.clone())
            .query(&[("page", page.to_string()), ("per_page", "100".to_string())])
            .send()
            .await?;

        let status = response.status();
        let response_text = response.text().await?;

        if !status.is_success() {
            return Err(eyre!("GitHub API error ({}): {}", status, response_text));
        }

        let response_json: Vec<Value> = serde_json::from_str(&response_text)
            .map_err(|e| eyre!("Error decoding response body: {}\nRaw response: {}", e, response_text))?;

        if response_json.is_empty() {
            break;
        }

        for repo in response_json {
            if archived || !repo["archived"].as_bool().unwrap_or(false) {
                if let (Some(repo_name), Some(created_at)) = (repo["full_name"].as_str(), repo["created_at"].as_str()) {
                    let date = created_at[..10].to_string();
                    repo_data.push((repo_name.to_owned(), date));
                }
            }
        }
        page += 1;
    }

    repo_data.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(repo_data)
}
