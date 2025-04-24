use clap::Parser;
use eyre::{Context, Result};
use regex::Regex;
use serde_yaml::{Mapping, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::{exit, Command},
};

#[derive(Parser)]
#[command(name = "ls-owners", about = "List CODEOWNERS and detect un-owned code paths")]
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
        // 1️⃣ Locate repo root and slug
        let (root_path, slug) = find_repo_root_and_slug(path_str)?;

        // 2️⃣ Load CODEOWNERS entries
        match load_ownership(&root_path)? {
            Ownership::Missing => {
                out.insert(slug, Value::String("MISSING_CODEOWNERS".into()));
                exit_code = 1;
                continue;
            }
            Ownership::Empty => {
                out.insert(slug, Value::String("EMPTY_CODEOWNERS".into()));
                exit_code = 1;
                continue;
            }
            Ownership::Present(entries) => {
                // 3️⃣ Gather all code files
                let code_files = gather_code_files(&root_path)?;

                // 4️⃣ Determine which top‐level dirs are un-owned
                let unowned = determine_unowned_paths(&entries, &code_files);

                // 5️⃣ Build the YAML mapping for this repo
                let mapping = build_repo_mapping(entries, unowned);
                out.insert(slug, Value::Mapping(mapping));
            }
        }
    }

    // 6️⃣ Emit final YAML and exit
    print_yaml_and_exit(out, exit_code);
}

/// Finds the repo root (via `git rev-parse`) and parses `origin` → `org/repo`.
fn find_repo_root_and_slug(path_str: &str) -> Result<(PathBuf, String)> {
    let repo_dir = PathBuf::from(path_str);
    let root = Command::new("git")
        .current_dir(&repo_dir)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("git rev-parse failed")?;
    if !root.status.success() {
        eyre::bail!("Not inside a Git repository at '{}'", path_str);
    }
    let repo_root = PathBuf::from(String::from_utf8(root.stdout)?.trim_end().to_string());

    let url_out = Command::new("git")
        .current_dir(&repo_dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .context("git remote get-url failed")?;
    let url = String::from_utf8(url_out.stdout)?.trim().to_string();
    let slug = parse_slug(&url).unwrap_or_else(|| "unknown/unknown".into());

    Ok((repo_root, slug))
}

enum Ownership {
    Missing,
    Empty,
    Present(BTreeMap<String, Vec<String>>),
}

/// Loads and parses `.github/CODEOWNERS`, classifying Missing, Empty, or Present(entries).
fn load_ownership(root: &Path) -> Result<Ownership> {
    let codeowners = root.join(".github").join("CODEOWNERS");
    if !codeowners.exists() {
        return Ok(Ownership::Missing);
    }

    let content = fs::read_to_string(&codeowners)
        .wrap_err_with(|| format!("Failed to read {}", codeowners.display()))?;
    let re_comment = Regex::new(r"^\s*#").unwrap();
    let mut entries = BTreeMap::<String, Vec<String>>::new();

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() || re_comment.is_match(line) {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        let pat = if parts[0] == "*" { "/" } else { parts[0] }.to_string();
        let owners = parts[1..]
            .iter()
            .map(|s| s.trim_start_matches('@').to_string())
            .collect();
        entries.insert(pat, owners);
    }

    if entries.is_empty() {
        Ok(Ownership::Empty)
    } else {
        Ok(Ownership::Present(entries))
    }
}

/// Recursively finds all “code” files under `root`, skipping `.git` and `.github`.
fn gather_code_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).wrap_err("Reading directory failed")? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if &name == ".git" || &name == ".github" {
                continue;
            }
            files.extend(gather_code_files(&path)?);
        } else if path.is_file() && is_code_file(&path) {
            files.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    Ok(files)
}

/// Given parsed ownership entries and a list of code files (relative paths),
/// returns the set of top‐level directories (or `/`) that aren’t covered.
fn determine_unowned_paths(
    entries: &BTreeMap<String, Vec<String>>,
    code_files: &[PathBuf],
) -> BTreeSet<String> {
    let mut unowned = BTreeSet::new();
    for rel in code_files {
        let s = format!("/{}", rel.to_string_lossy());
        let covered = entries.keys().any(|pat| s.starts_with(pat));
        if !covered {
            let comps: Vec<&str> = s.split('/').filter(|c| !c.is_empty()).collect();
            let dir = if comps.len() <= 1 {
                "/".to_string()
            } else {
                format!("/{}/", comps[0])
            };
            unowned.insert(dir);
        }
    }
    unowned
}

/// Builds the `serde_yaml::Mapping` for a repo:
/// each path → owner(s) or `"UNOWNED"`, ordered with `/` first, then by path depth, then lexically.
fn build_repo_mapping(
    entries: BTreeMap<String, Vec<String>>,
    unowned: BTreeSet<String>,
) -> Mapping {
    // 1. Merge keys
    let mut all_keys: Vec<String> = entries.keys().cloned().collect();
    for dir in &unowned {
        if !entries.contains_key(dir) {
            all_keys.push(dir.clone());
        }
    }

    // 2. Sort with custom comparator
    all_keys.sort_by(|a, b| {
        // "/" always first
        if a == "/" && b != "/" {
            return std::cmp::Ordering::Less;
        }
        if b == "/" && a != "/" {
            return std::cmp::Ordering::Greater;
        }
        // compare by number of segments ("/foo/bar/" → 2), fewer first
        let depth = |s: &str| s.trim_matches('/').split('/').filter(|p| !p.is_empty()).count();
        let da = depth(a);
        let db = depth(b);
        match da.cmp(&db) {
            std::cmp::Ordering::Equal => a.cmp(b),
            other => other,
        }
    });

    // 3. Build mapping in sorted order
    let mut map = Mapping::new();
    for key in all_keys {
        let value = if let Some(owners) = entries.get(&key) {
            match owners.len() {
                0 => Value::String("UNOWNED".into()), // should not happen
                1 => Value::String(owners[0].clone()),
                _ => {
                    let seq = owners.iter().cloned().map(Value::String).collect();
                    Value::Sequence(seq)
                }
            }
        } else {
            Value::String("UNOWNED".into())
        };
        map.insert(Value::String(key), value);
    }

    map
}

/// Prints the final YAML map and exits with the given code.
fn print_yaml_and_exit(map: BTreeMap<String, Value>, code: i32) -> ! {
    let s = serde_yaml::to_string(&map).unwrap();
    print!("{s}");
    exit(code);
}

/// Parses GitHub origin URLs into `org/repo`, supporting both SSH and HTTPS.
fn parse_slug(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        Some(rest.trim_end_matches(".git").to_string())
    } else if let Some(rest) = url.strip_prefix("https://github.com/") {
        Some(rest.trim_end_matches(".git").to_string())
    } else {
        None
    }
}

/// Heuristic: treat certain extensions and filenames as “code”.
fn is_code_file(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name == "Dockerfile" || name == "Makefile" {
            return true;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) {
            return matches!(
                ext.as_str(),
                "py" | "js" | "jsx" | "ts" | "tsx" | "css"
                    | "html" | "tf" | "yaml" | "yml" | "toml" | "tpl"
            );
        }
    }
    false
}
