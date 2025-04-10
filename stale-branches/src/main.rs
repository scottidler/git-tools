use clap::Parser;
use env_logger;
use eyre::{Result, Context};
use log::debug;
use serde::Serialize;
use serde_yaml;
use std::collections::HashMap;
use std::io::{self, Write};
use std::process::Command;
use chrono::{Utc, NaiveDate};

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/git_describe.rs"));
}

#[derive(Parser, Debug)]
#[command(name = "stale-branches", about = "Generate a YAML report of stale branches.")]
#[command(version = built_info::GIT_DESCRIBE)]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[arg(help = "Number of days to consider a branch stale.")]
    days: i64,

    #[arg(long, help = "Git reference to check.", default_value = "refs/remotes/origin")]
    ref_: String,
}

#[derive(Serialize, Debug)]
struct AuthorBranches {
    branches: Vec<HashMap<String, i64>>,
    count: usize,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();

    Command::new("git")
        .args(["fetch", "origin", "--prune"])
        .output()
        .wrap_err("Failed to prune local cache of git branches")?;

    let branches = get_stale_branches(args.days, &args.ref_)?;
    generate_yaml(&branches)?;

    Ok(())
}

fn get_stale_branches(days: i64, ref_: &str) -> Result<Vec<(String, i64, String)>> {
    let output = Command::new("git")
        .args(["for-each-ref", "--sort=-committerdate", ref_, "--format=%(committerdate:short) %(refname:short) %(committername)"])
        .output()
        .wrap_err("Failed to execute git command")?;

    let current_time = Utc::now().timestamp();
    debug!("current_time: {}", current_time);
    let result = String::from_utf8(output.stdout)?;

    let branches: Vec<(String, i64, String)> = result.lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 { return None; }
            let date_str = parts[0];
            let branch = parts[1].trim_start_matches("origin/").to_string();
            let author = parts[2..].join(" ");
            let commit_time = NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
                .ok()?
                .and_hms_opt(0, 0, 0)?
                .and_utc().timestamp();
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

fn generate_yaml(branches: &[(String, i64, String)]) -> Result<()> {
    let mut authors_dict: HashMap<String, AuthorBranches> = HashMap::new();

    for (branch, days, author) in branches {
        authors_dict
            .entry(author.clone())
            .or_insert_with(|| AuthorBranches { branches: vec![], count: 0 })
            .branches
            .push(HashMap::from([(branch.clone(), *days)]));
        authors_dict.get_mut(author).unwrap().count += 1;
    }

    let yaml_data = serde_yaml::to_string(&authors_dict).wrap_err("Failed to serialize data to YAML")?;
    io::stdout().write_all(yaml_data.as_bytes()).wrap_err("Failed to write YAML to stdout")?;

    Ok(())
}
