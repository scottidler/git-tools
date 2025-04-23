use clap::Parser;
use eyre::{Context, Result};
use regex::Regex;
use serde_yaml::{Mapping, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    process::{exit, Command},
};

#[derive(Parser)]
#[command(name = "ls-owners", about = "List CODEOWNERS entries for local Git repos")]
struct Cli {
    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut out = BTreeMap::<String, Value>::new();
    let mut exit_code = 0;

    for path_str in &cli.paths {
        let repo_dir = PathBuf::from(path_str);

        // 1. Find repo root
        let root = Command::new("git")
            .current_dir(&repo_dir)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .context("git rev-parse failed")?;
        if !root.status.success() {
            eyre::bail!("Not inside a Git repository at '{}'", path_str);
        }
        let repo_root = String::from_utf8(root.stdout)?
            .trim_end()
            .to_string();

        // 2. Derive slug
        let url_out = Command::new("git")
            .current_dir(&repo_dir)
            .args(["remote", "get-url", "origin"])
            .output()
            .context("git remote get-url failed")?;
        let url = String::from_utf8(url_out.stdout)?
            .trim()
            .to_string();
        let slug = parse_slug(&url).unwrap_or_else(|| "unknown/unknown".into());

        // 3. Locate CODEOWNERS
        let codeowners = PathBuf::from(&repo_root)
            .join(".github")
            .join("CODEOWNERS");

        // 4a. Missing file?
        if !codeowners.exists() {
            out.insert(slug, Value::String("MISSING_CODEOWNERS".into()));
            exit_code = 1;
            continue;
        }

        // 4b. Read & parse
        let content = fs::read_to_string(&codeowners)
            .wrap_err_with(|| format!("Failed to read {}", codeowners.display()))?;
        let mut entries = BTreeMap::<String, Vec<String>>::new();
        let re_comment = Regex::new(r"^\s*#").unwrap();

        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || re_comment.is_match(line) {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let path = if parts[0] == "*" { "/" } else { parts[0] }.to_string();
            let owners = parts[1..]
                .iter()
                .map(|s| s.trim_start_matches('@').to_string())
                .collect();
            entries.insert(path, owners);
        }

        // 4c. Empty?
        if entries.is_empty() {
            out.insert(slug, Value::String("EMPTY_CODEOWNERS".into()));
            exit_code = 1;
            continue;
        }

        // 5. Build slug → (path → [owners…])
        let mut mapping = Mapping::new();
        for (path, owners) in entries {
            let seq = owners.into_iter().map(Value::String).collect::<Vec<_>>();
            mapping.insert(Value::String(path), Value::Sequence(seq));
        }
        out.insert(slug, Value::Mapping(mapping));
    }

    // 6. Emit & exit
    let yaml = serde_yaml::to_string(&out).expect("YAML serialization failed");
    print!("{yaml}");
    exit(exit_code);
}

/// Support SSH or HTTPS GitHub URLs → "org/repo"
fn parse_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        Some(rest.trim_end_matches(".git").to_string())
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        Some(rest.trim_end_matches(".git").to_string())
    } else {
        None
    }
}
