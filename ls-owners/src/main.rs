use clap::Parser;
use eyre::{Context, Result};
use regex::Regex;
use serde_yaml::{Mapping, Value};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, USER_AGENT};
use serde_json::Value as JsonValue;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    env,
    fs,
    path::{Path, PathBuf},
    process::{exit, Command},
};
use rayon::prelude::*;
use colored::Colorize;

// for base64 decoding:
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use std::sync::{Arc, Mutex};

const TOP_AUTHORS: usize = 5;

enum Ownership {
    Missing,
    Empty,
    Present(BTreeMap<String, Vec<String>>),
}

/// Holds each repository’s slug, its status, and the YAML value to print.
struct Repo {
    slug: String,
    status: String,
    value: Value,
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

    /// Show detailed output (full YAML-style listing)
    #[arg(short = 'd', long = "detailed")]
    detailed: bool,

    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

fn main() -> Result<()> {
    // 1) Parse CLI
    let cli = Cli::parse();

    // 2) Grab GitHub token
    let token = get_github_token()?;

    // 3) Build a blocking reqwest client with default headers
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("ls-owners"));
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("token {}", token))?,
    );
    let client = Client::builder()
        .default_headers(headers)
        .build()
        .context("failed to build HTTP client")?;

    // 4) Load our on-disk ETag cache into a thread-safe mutex
    let initial_cache = load_etag_cache()?;
    let etag_cache = Arc::new(Mutex::new(initial_cache));

    // 5) List all repositories in your org (change "your-org" to your real org login)
    let repos: Vec<String> = list_github_repos("your-org", &client)
        .context("failed to list GitHub repositories")?;

    // 6) Fetch CODEOWNERS & build Repo items in parallel
    let results: Vec<Repo> = repos
        .into_par_iter()
        .filter_map(|slug| {
            // a) Pull the fetched ownership, updating the shared ETag cache
            let ownership = {
                let mut cache_lock = etag_cache.lock().unwrap();
                match fetch_remote_codeowners(&slug, &client, &mut *cache_lock) {
                    Ok(o) => o,
                    Err(err) => {
                        eprintln!("❌ {}: {}", slug, err);
                        return None;
                    }
                }
            };

            // b) Restore ex-employees logic here
            let exclude = match read_ex_employees(slug.split('/').next().unwrap_or("unknown")) {
                Ok(set) => set,
                Err(err) => {
                    eprintln!("⚠️ could not read ex-employees for {}: {}", slug, err);
                    BTreeSet::new()
                }
            };

            // c) Build the Repo struct, filtering by ex-employees as needed
            match try_build_repo(&slug, ownership, &exclude) {
                Ok(Some(repo)) => Some(repo),
                Ok(None) => None,
                Err(err) => {
                    eprintln!("❌ {}: {}", slug, err);
                    None
                }
            }
        })
        .collect();

    // 7) Sort and print
    let sorted = sorted_entries(&results);
    if cli.detailed {
        print_detailed(&sorted);
    } else {
        print_simplified(&sorted);
    }

    // 8) Persist updated ETags
    let final_cache = Arc::try_unwrap(etag_cache)
        .expect("multiple Arc references to cache")
        .into_inner()
        .unwrap();
    save_etag_cache(&final_cache).context("failed to save ETag cache")?;

    // 9) Exit with non-zero if any repo was not fully owned
    let exit_code = if results.iter().any(|r| r.status != "owned") {
        1
    } else {
        0
    };
    exit(exit_code);
}

/// Reads ex-employees for the given org from `~/.config/ls-owners/{org}/ex-employees`
fn read_ex_employees(org: &str) -> eyre::Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    if let Some(mut cfg) = dirs::config_dir() {
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

/// Finds all Git repositories under the given paths:
/// - If a path itself has a `.git` folder, it’s treated as a repo root.
/// - Otherwise it scans first-level subdirectories for `.git`.
/// - For any first-level subdirectory that isn’t a repo, it also scans its immediate children,
///   to pick up structures like `./org/<repo>`.
fn find_repo_paths(paths: &[String]) -> eyre::Result<Vec<PathBuf>> {
    let mut repos = Vec::new();

    for p in paths {
        let pb = PathBuf::from(p);

        if pb.join(".git").is_dir() {
            repos.push(pb.clone());
            continue;
        }

        if pb.is_dir() {
            for entry in fs::read_dir(&pb).context("reading directory")? {
                let entry = entry?;
                let child = entry.path();

                if child.join(".git").is_dir() {
                    repos.push(child.clone());
                    continue;
                }

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

fn try_process_repo(
    root_path: &PathBuf,
    filter_set: &Option<BTreeSet<String>>,
) -> Result<Option<(String, String, Mapping)>> {
    let (repo_root, slug) = find_repo_root_and_slug(root_path.to_str().unwrap())?;
    let exclude = read_ex_employees(&slug.split('/').next().unwrap_or("unknown"))?;

    let (status, mapping, _) = match load_ownership(&repo_root)? {
        Ownership::Missing => {
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::String("MISSING_CODEOWNERS".into()),
            );
            let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned".to_string(), m, true)
        }
        Ownership::Empty => {
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::String("EMPTY_CODEOWNERS".into()),
            );
            let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned".to_string(), m, true)
        }
        Ownership::Present(entries) => {
            let code_files = gather_code_files(&repo_root)?;
            let unowned_dirs = determine_unowned_paths(&entries, &code_files);
            let computed_status = if unowned_dirs.is_empty() {
                "owned"
            } else {
                "partial"
            };
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::Mapping(build_repo_mapping(entries, unowned_dirs)),
            );

            let has_authors = computed_status != "owned";
            if has_authors {
                let authors = get_top_authors(&repo_root, TOP_AUTHORS, &exclude)?;
                let seq = authors.into_iter().map(Value::String).collect();
                m.insert(Value::String("authors".into()), Value::Sequence(seq));
            }

            (computed_status.to_string(), m, has_authors)
        }
    };

    if let Some(filters) = filter_set {
        if !filters.contains(&status.to_lowercase()) {
            return Ok(None);
        }
    }

    Ok(Some((slug, status, mapping)))
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
    let mut all_keys: Vec<String> = entries.keys().cloned().collect();
    for dir in &unowned {
        if !entries.contains_key(dir) {
            all_keys.push(dir.clone());
        }
    }

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

/// Sort by status (unowned < partial < owned), then by slug
fn sorted_entries(results: &[Repo]) -> Vec<&Repo> {
    let mut refs: Vec<&Repo> = results.iter().collect();

    fn rank(s: &str) -> usize {
        match s {
            "unowned" => 0,
            "partial" => 1,
            "owned"   => 2,
            _         => 3,
        }
    }

    refs.sort_by(|a, b| {
        rank(&a.status)
            .cmp(&rank(&b.status))
            .then_with(|| a.slug.cmp(&b.slug))
    });

    refs
}

/// Simplified: color + status on left, two spaces, then slug.
fn print_simplified(entries: &[&Repo]) {
    let width = "unowned".len();

    for r in entries {
        let colored = match r.status.as_str() {
            "owned"   => r.status.green().bold(),
            "partial" => r.status.yellow().bold(),
            "unowned" => r.status.red().bold(),
            other     => other.normal(),
        };
        let pad = format!("{:>width$}", colored, width = width);
        println!("{} {}", pad, r.slug);
    }

    println!("count {}", entries.len());
}

/// Detailed: status + slug on one line (no buffer), then YAML-style indented under it.
fn print_detailed(entries: &[&Repo]) {
    for r in entries {
        let colored = match r.status.as_str() {
            "owned"   => r.status.green().bold(),
            "partial" => r.status.yellow().bold(),
            "unowned" => r.status.red().bold(),
            other     => other.normal(),
        };

        println!("{} {}:", colored, r.slug);

        match &r.value {
            Value::String(s) => {
                println!("  {}", s);
            }
            Value::Mapping(m) => {
                if let Some(Value::Mapping(paths)) = m.get(&Value::String("paths".into())) {
                    println!("  paths:");
                    for (p, owners) in paths {
                        let path = p.as_str().unwrap_or_default();
                        match owners {
                            Value::Sequence(seq) => {
                                let list: Vec<&str> =
                                    seq.iter().filter_map(Value::as_str).collect();
                                if list.len() == 1 {
                                    println!("    {}: {}", path, list[0]);
                                } else {
                                    println!("    {}: [{}]", path, list.join(", "));
                                }
                            }
                            Value::String(s2) => {
                                println!("    {}: {}", path, s2);
                            }
                            _ => {
                                println!("    {}: {:?}", path, owners);
                            }
                        }
                    }
                }
                if let Some(Value::Sequence(authors)) = m.get(&Value::String("authors".into())) {
                    println!("  authors:");
                    for a in authors {
                        if let Some(name) = a.as_str() {
                            println!("    - {}", name);
                        }
                    }
                }
            }
            other => {
                println!("  {:?}", other);
            }
        }
    }

    println!("Matched {} repos", entries.len());
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

/// Load the ETag map from `~/.config/ls-owners/cache.json`.
fn load_etag_cache() -> eyre::Result<HashMap<String, String>> {
    let mut cfg = dirs::config_dir()
        .ok_or_else(|| eyre::eyre!("couldn’t find config directory"))?;
    cfg.push("ls-owners");
    fs::create_dir_all(&cfg)?;
    cfg.push("cache.json");

    if !cfg.is_file() {
        return Ok(HashMap::new());
    }
    let data = fs::read_to_string(&cfg)?;
    let map: HashMap<String, String> = serde_json::from_str(&data)?;
    Ok(map)
}

/// Save the ETag map back out to `~/.config/ls-owners/cache.json`.
fn save_etag_cache(cache: &HashMap<String, String>) -> eyre::Result<()> {
    let mut cfg = dirs::config_dir()
        .ok_or_else(|| eyre::eyre!("couldn’t find config directory"))?;
    cfg.push("ls-owners");
    fs::create_dir_all(&cfg)?;
    cfg.push("cache.json");

    let data = serde_json::to_string_pretty(cache)?;
    fs::write(&cfg, data)?;
    Ok(())
}

/// Pull GITHUB_TOKEN or GH_TOKEN from the environment, or fail.
fn get_github_token() -> eyre::Result<String> {
    env::var("GITHUB_TOKEN")
        .or_else(|_| env::var("GH_TOKEN"))
        .map_err(|_| eyre::eyre!("GitHub token missing; set GITHUB_TOKEN or GH_TOKEN"))
}

/// List all repo slugs ("org/name") in the given org via the REST API.
fn list_github_repos(org: &str, client: &Client) -> eyre::Result<Vec<String>> {
    let mut page = 1;
    let mut all = Vec::new();

    loop {
        let url = format!(
            "https://api.github.com/orgs/{org}/repos?per_page=100&page={page}",
            org = org,
            page = page
        );
        let resp = client
            .get(&url)
            .send()?
            .error_for_status()?;
        let repos: Vec<JsonValue> = resp.json()?;
        if repos.is_empty() {
            break;
        }
        for repo in &repos {
            if let Some(full) = repo
                .get("full_name")
                .and_then(JsonValue::as_str)
            {
                all.push(full.to_string());
            }
        }
        if repos.len() < 100 {
            break;
        }
        page += 1;
    }

    Ok(all)
}

/// Try to fetch `.github/CODEOWNERS` from GitHub for `slug` ("org/repo").
/// Uses If-None-Match if we have a previous ETag in `cache`.
/// Updates `cache` with the new ETag (or leaves it on 304).
fn fetch_remote_codeowners(
    slug: &str,
    client: &Client,
    cache: &mut HashMap<String, String>,
) -> eyre::Result<Ownership> {
    // read ex-employees for this org
    let org = slug.split('/').next().unwrap_or("unknown");
    let exclude = read_ex_employees(org)?;

    // build the URL
    let url = format!(
        "https://api.github.com/repos/{slug}/contents/.github/CODEOWNERS",
        slug = slug
    );
    let mut req = client.get(&url);
    if let Some(etag) = cache.get(slug) {
        req = req.header("If-None-Match", etag);
    }

    let resp = req.send()?.error_for_status()?;
    match resp.status() {
        reqwest::StatusCode::NOT_MODIFIED => {
            // 304 → no change
            Ok(Ownership::Present(BTreeMap::new()))
        }

        reqwest::StatusCode::OK => {
            // update ETag
            if let Some(new_etag) = resp.headers().get("etag") {
                cache.insert(slug.to_string(), new_etag.to_str()?.to_string());
            }

            // parse base64-encoded content
            let json: JsonValue = resp.json()?;
            let content_b64 = json
                .get("content")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| eyre::eyre!("no content field in GitHub response"))?;
            let decoded = STANDARD.decode(content_b64.replace('\n', ""))?;
            let text = String::from_utf8(decoded)?;

            // build the entries map
            let re_comment = Regex::new(r"^\s*#")?;
            let mut entries: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || re_comment.is_match(line) {
                    continue;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() < 2 {
                    continue;
                }
                let pat = if parts[0] == "*" { "/" } else { parts[0] }.to_string();
                let mut owners: Vec<String> = parts[1..]
                    .iter()
                    .map(|s| s.trim_start_matches('@').to_string())
                    .collect();

                // filter out any ex-employees
                owners.retain(|o| !exclude.contains(o));

                entries.insert(pat, owners);
            }

            if entries.is_empty() {
                Ok(Ownership::Empty)
            } else {
                Ok(Ownership::Present(entries))
            }
        }

        status if status == reqwest::StatusCode::NOT_FOUND => {
            // 404 → no CODEOWNERS
            Ok(Ownership::Missing)
        }

        other => {
            eyre::bail!("GitHub returned {} for {}", other, slug);
        }
    }
}

/// Given a slug, its fetched Ownership, and the set of ex-employees to exclude,
/// produce a Repo (or None if filtered out).
fn try_build_repo(
    slug: &str,
    ownership: Ownership,
    exclude: &BTreeSet<String>,
) -> Result<Option<Repo>> {
    // For “present” entries, we need to filter out any owners who are ex-employees.
    // Then we build the YAML mapping and decide status.
    let (status, mapping) = match ownership {
        Ownership::Missing => {
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::String("MISSING_CODEOWNERS".into()),
            );
            ("unowned".to_string(), m)
        }
        Ownership::Empty => {
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::String("EMPTY_CODEOWNERS".into()),
            );
            ("unowned".to_string(), m)
        }
        Ownership::Present(mut entries) => {
            // 1) Remove any @owner who is in the ex-employees list
            for owners in entries.values_mut() {
                owners.retain(|o| !exclude.contains(o));
            }

            // 2) Build the YAML mapping exactly as before
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::Mapping(build_repo_mapping(entries.clone(), BTreeSet::new())),
            );

            // 3) Decide status: if there are no UNOWNED paths, mark as "owned"
            let has_unowned = matches!(
                m.get(&Value::String("paths".into())),
                Some(Value::Mapping(pm)) if pm.values().any(|v| matches!(v, Value::String(s) if s == "UNOWNED"))
            );
            let computed_status = if has_unowned { "partial" } else { "owned" };

            (computed_status.to_string(), m)
        }
    };

    Ok(Some(Repo {
        slug: slug.to_string(),
        status,
        value: Value::Mapping(mapping),
    }))
}
