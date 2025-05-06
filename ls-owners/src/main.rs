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
use rayon::prelude::*;

const TOP_AUTHORS: usize = 5;

/// Reads ex-employees for the given org from `~/.config/ls-owners/{org}/ex-employees`
fn read_ex_employees(org: &str) -> eyre::Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    if let Some(mut cfg) = dirs::config_dir() {
        // note: use "ls-owners" to match your actual config directory
        cfg.push("ls-owners");
        cfg.push(org);
        cfg.push("ex-employees");
        if let Ok(data) = fs::read_to_string(&cfg) {
            for line in data.lines() {
                let name = line.trim();
                if !name.is_empty() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    Ok(set)
}

#[derive(Parser)]
#[command(name = "ls-owners", about = "List CODEOWNERS and detect un-owned code paths")]
struct Cli {
    /// Only show repos with these statuses: owned, unowned, partial
    #[arg(
        short = 'o',
        long = "only",
        value_name = "FILTER",
        num_args = 1..,
        value_parser = ["owned", "unowned", "partial"]
    )]
    only: Vec<String>,

    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Prepare optional filter set
    let filter_set: Option<BTreeSet<String>> = if cli.only.is_empty() {
        None
    } else {
        Some(cli.only.iter().map(|s| s.to_lowercase()).collect())
    };

    // ② discover all git-repo roots under the CLI paths
    let repo_dirs = find_repo_paths(&cli.paths)
        .context("failed to scan for repositories")?;

    // ③ process each repo in parallel, collecting only those with actual output:
    let results: Vec<(String, Value)> = repo_dirs
        .par_iter()
        .filter_map(|root_path| {
            // duplicate of your per-repo logic, but returning Option
            match try_process_repo(root_path, &filter_set) {
                Ok(Some((slug_status, mapping))) => Some((slug_status, Value::Mapping(mapping))),
                Ok(None) => None, // filtered out or fully owned with --only
                Err(err) => {
                    eprintln!("❌ {}: {}", root_path.display(), err);
                    None
                }
            }
        })
        .collect();

    // assemble into a BTreeMap so we can use your existing printer
    let mut out = BTreeMap::new();
    let exit_code = results.iter().any(|(_, v)| {
        // any Unowned/Empty => nonzero exit
        if let Value::Mapping(m) = v {
            // inspect v or keep track in try_process_repo
            m.contains_key(&Value::String("authors".into()))
        } else {
            false
        }
    }).then(|| 1).unwrap_or(0);

    for (k, v) in results {
        out.insert(k, v);
    }

    print_manual_yaml_and_exit(&out, exit_code);
}

/// Finds all Git repositories under the given paths:
/// - If a path itself has a `.git` folder, it’s treated as a repo root.
/// - Otherwise it scans first-level subdirectories for `.git`.
/// - For any first-level subdirectory that isn’t a repo, it also scans its immediate children,
///   to pick up structures like `./org/<repo>`.
fn find_repo_paths(paths: &[String]) -> eyre::Result<Vec<PathBuf>> {
    let mut repos = Vec::new();

    for p in paths {
        let pb = PathBuf::from(p);

        // 1) If this path is itself a repo root, include it.
        if pb.join(".git").is_dir() {
            repos.push(pb.clone());
            continue;
        }

        // 2) Otherwise, if it’s a directory, scan its children.
        if pb.is_dir() {
            for entry in fs::read_dir(&pb).context("reading directory")? {
                let entry = entry?;
                let child = entry.path();

                // 2a) If child is a repo, include it.
                if child.join(".git").is_dir() {
                    repos.push(child.clone());
                    continue;
                }

                // 2b) Otherwise, if the child is a directory, scan *its* immediate children.
                if child.is_dir() {
                    for subentry in fs::read_dir(&child).context("reading subdirectory")? {
                        let subentry = subentry?;
                        let sub = subentry.path();
                        if sub.join(".git").is_dir() {
                            repos.push(sub);
                        }
                    }
                }
            }
        }
    }

    Ok(repos)
}

// Extracted per-repo logic, return None if should be skipped entirely:
fn try_process_repo(
    root_path: &PathBuf,
    filter_set: &Option<BTreeSet<String>>,
) -> Result<Option<(String, Mapping)>> {
    // Determine actual git root and "org/repo" slug
    let (repo_root, slug) = find_repo_root_and_slug(root_path.to_str().unwrap())?;
    let exclude = read_ex_employees(&slug.split('/').next().unwrap_or("unknown"))?;

    // We don't need the third element beyond matching on status & mapping
    let (status, mapping, _) = match load_ownership(&repo_root)? {
        Ownership::Missing => {
            let mut m = Mapping::new();
            m.insert(Value::String("paths".into()), Value::String("MISSING_CODEOWNERS".into()));
            let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned", m, true)
        }
        Ownership::Empty => {
            let mut m = Mapping::new();
            m.insert(Value::String("paths".into()), Value::String("EMPTY_CODEOWNERS".into()));
            let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned", m, true)
        }
        Ownership::Present(entries) => {
            let code_files = gather_code_files(&repo_root)?;
            let unowned_dirs = determine_unowned_paths(&entries, &code_files);
            let status = if unowned_dirs.is_empty() { "owned" } else { "partial" };
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::Mapping(build_repo_mapping(entries, unowned_dirs)),
            );

            // include authors only for partial/unowned
            let mut has_authors = false;
            if status != "owned" {
                let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
                let seq = authors.into_iter().map(Value::String).collect();
                m.insert(Value::String("authors".into()), Value::Sequence(seq));
                has_authors = true;
            }
            (status, m, has_authors)
        }
    };

    // Apply `--only` filtering if requested
    if let Some(filters) = filter_set {
        if !filters.contains(&status.to_lowercase()) {
            return Ok(None);
        }
    }

    let key = format!("{slug} ({status})");
    Ok(Some((key, mapping)))
}

/// Runs `git shortlog -s -n --all --no-merges` and returns up to `limit` authors,
/// filtering out any whose full name appears in `exclude`.
fn get_top_authors(
    repo: &Path,
    limit: usize,
    exclude: &BTreeSet<String>,
) -> Result<Vec<String>> {
    let output = Command::new("git")
        .current_dir(repo)
        .args(&["shortlog", "-s", "-n", "--all", "--no-merges"])
        .output()
        .context("git shortlog failed")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8(output.stdout)?;
    let mut authors = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        let mut parts = line.split_whitespace();
        if let Some(count) = parts.next() {
            let name = parts.collect::<Vec<_>>().join(" ");
            if exclude.contains(&name) {
                continue;
            }
            authors.push(format!("{name} ({count})"));
            if authors.len() == limit {
                break;
            }
        }
    }
    Ok(authors)
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
/// each path → owner(s) or `"UNOWNED"`, in the desired order.
fn build_repo_mapping(
    entries: BTreeMap<String, Vec<String>>,
    unowned: BTreeSet<String>,
) -> Mapping {
    // 1. Collect all keys (owned + unowned)
    let mut all_keys: Vec<String> = entries.keys().cloned().collect();
    for dir in &unowned {
        if !entries.contains_key(dir) {
            all_keys.push(dir.clone());
        }
    }

    // 2. Sort: "/" first, then by segment count (shallow → deep), then lexicographically
    all_keys.sort_by(|a, b| {
        if a == "/" && b != "/" {
            return std::cmp::Ordering::Less;
        }
        if b == "/" && a != "/" {
            return std::cmp::Ordering::Greater;
        }
        let depth = |s: &str| s.trim_matches('/').split('/').filter(|p| !p.is_empty()).count();
        match depth(a).cmp(&depth(b)) {
            std::cmp::Ordering::Equal => a.cmp(b),
            other => other,
        }
    });

    // 3. Build mapping in that order
    let mut map = Mapping::new();
    for key in all_keys {
        let val = if let Some(owners) = entries.get(&key) {
            match owners.len() {
                0 => Value::String("UNOWNED".into()),
                1 => Value::String(owners[0].clone()),
                _ => {
                    let seq = owners.iter().cloned().map(Value::String).collect();
                    Value::Sequence(seq)
                }
            }
        } else {
            Value::String("UNOWNED".into())
        };
        map.insert(Value::String(key), val);
    }
    map
}

/// Print our nested YAML in block style, then print a count of matched repos and exit:
/// - “paths” is a nested mapping
/// - “authors” is always a block list of “Name (count)”
/// - After all repos are printed, it prints “Matched X repos”
fn print_manual_yaml_and_exit(map: &BTreeMap<String, Value>, code: i32) -> ! {
    for (repo, val) in map {
        match val {
            Value::String(s) => {
                println!("{repo}: {s}");
            }
            Value::Mapping(m) => {
                println!("{repo}:");
                for (k, v) in m {
                    let key = k.as_str().unwrap_or_default();
                    match v {
                        Value::Mapping(paths_m) if key == "paths" => {
                            println!("  paths:");
                            for (p_k, p_v) in paths_m {
                                let path = p_k.as_str().unwrap_or_default();
                                match p_v {
                                    Value::Sequence(seq) => {
                                        let owners: Vec<&str> =
                                            seq.iter().filter_map(Value::as_str).collect();
                                        if owners.len() == 1 {
                                            println!("    {path}: {}", owners[0]);
                                        } else {
                                            println!("    {path}: [{}]", owners.join(", "));
                                        }
                                    }
                                    Value::String(s2) => {
                                        println!("    {path}: {s2}");
                                    }
                                    _ => {
                                        println!("    {path}: {p_v:?}");
                                    }
                                }
                            }
                        }
                        Value::Sequence(authors) if key == "authors" => {
                            println!("  authors:");
                            for author in authors {
                                if let Some(name) = author.as_str() {
                                    println!("    - {name}");
                                }
                            }
                        }
                        _ => {
                            // fallback for other unexpected entries
                            println!("  {key}: {v:?}");
                        }
                    }
                }
            }
            other => {
                println!("{repo}: {other:?}");
            }
        }
    }

    // Summary line: count of repos that were printed
    println!("Matched {} repos", map.len());

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
