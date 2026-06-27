use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::{ArgAction, Parser};
use colored::Colorize;
use common::parallel::ParallelExecutor;
use common::repo::RepoDiscovery;
use eyre::{Context, Result, bail, eyre};
use log::{LevelFilter, debug, warn};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::exit,
};

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
#[command(name = "ls-owners", about = "List CODEOWNERS and detect un-owned code paths", version = env!("GIT_DESCRIBE"))]
struct Cli {
    #[arg(short = 'l', long, default_value_t = LevelFilter::Info, help = "log level: error, warn, info, debug, trace")]
    log_level: LevelFilter,

    /// Only show repos with these statuses: owned, unowned, partial
    #[arg(
        short = 'o',
        long = "only",
        value_name = "FILTER",
        action = ArgAction::Append,
        value_parser = ["owned", "unowned", "partial"]
    )]
    only: Vec<String>,

    /// Show detailed output (full YAML-style listing)
    #[arg(short = 'd', long = "detailed")]
    detailed: bool,

    /// Scan repos REMOTELY via the GitHub API for the given org(s) or user(s)
    /// instead of local paths. Requires GITHUB_TOKEN or GH_TOKEN. Space-separated
    /// or repeated.
    #[arg(long = "org", value_name = "ORG_OR_USER", num_args = 1.., action = ArgAction::Append)]
    orgs: Vec<String>,

    /// One or more paths to Git repos (defaults to current directory)
    #[arg(value_name = "PATH", default_values = &["."], num_args = 0..)]
    paths: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    common::log::init(cli.log_level, "ls-owners")?;
    debug!(
        "main: only={:?} detailed={} orgs={:?} paths={:?}",
        cli.only, cli.detailed, cli.orgs, cli.paths
    );

    let filter_set: Option<BTreeSet<String>> = if cli.only.is_empty() {
        None
    } else {
        Some(cli.only.iter().map(|s| s.to_lowercase()).collect())
    };

    // --org selects REMOTE mode (scan via the GitHub API); otherwise scan the
    // local paths. They are mutually exclusive - --org wins when present.
    let results: Vec<Repo> = if cli.orgs.is_empty() {
        let discovery = RepoDiscovery::new(cli.paths);
        let repos = discovery.discover().context("failed to scan for repositories")?;
        let executor = ParallelExecutor::new(repos);
        executor.execute(|repo_info| match try_process_repo(repo_info, &filter_set) {
            Ok(Some((slug, status, mapping))) => Ok(Some(Repo {
                slug,
                status,
                value: Value::Mapping(mapping),
            })),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        })
    } else {
        scan_remote(&cli.orgs, &filter_set)?
    };

    let sorted = sorted_entries(&results);

    if cli.detailed {
        print_detailed(&sorted);
    } else {
        print_simplified(&sorted);
    }

    let exit_code = if results.iter().any(|r| r.status != "owned") { 1 } else { 0 };
    exit(exit_code);
}

/// XDG config dir, honoring `$XDG_CONFIG_HOME` and falling back to `$HOME/.config`.
///
/// We deliberately do NOT use the `dirs` config/data helpers: those honor
/// `$XDG_CONFIG_HOME` / `$XDG_DATA_HOME` only on Linux. On macOS they resolve via system
/// APIs and return `~/Library/...`, ignoring the env vars. These helpers resolve to the
/// same XDG layout on every platform.
fn xdg_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(dir);
        if path.is_absolute() {
            return Some(path);
        }
    }
    dirs::home_dir().map(|h| h.join(".config"))
}

/// Reads ex-employees for the given org from `~/.config/ls-owners/{org}/ex-employees`
fn read_ex_employees(org: &str) -> eyre::Result<BTreeSet<String>> {
    let mut set = BTreeSet::new();
    if let Some(mut cfg) = xdg_config_dir() {
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

fn try_process_repo(
    repo_info: &common::repo::RepoInfo,
    filter_set: &Option<BTreeSet<String>>,
) -> Result<Option<(String, String, Mapping)>> {
    let repo_root = &repo_info.path;
    let slug = &repo_info.slug;
    let exclude = read_ex_employees(slug.split('/').next().unwrap_or("unknown"))?;

    let (status, mapping, _) = match load_ownership(repo_root)? {
        Ownership::Missing => {
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::String("MISSING_CODEOWNERS".into()),
            );
            let authors = get_top_authors(repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned".to_string(), m, true)
        }
        Ownership::Empty => {
            let mut m = Mapping::new();
            m.insert(Value::String("paths".into()), Value::String("EMPTY_CODEOWNERS".into()));
            let authors = get_top_authors(repo_root, TOP_AUTHORS, &exclude)?;
            let seq = authors.into_iter().map(Value::String).collect();
            m.insert(Value::String("authors".into()), Value::Sequence(seq));
            ("unowned".to_string(), m, true)
        }
        Ownership::Present(entries) => {
            let code_files = gather_code_files(repo_root)?;
            let unowned_dirs = determine_unowned_paths(&entries, &code_files);
            let computed_status = if unowned_dirs.is_empty() { "owned" } else { "partial" };
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::Mapping(build_repo_mapping(entries, unowned_dirs)),
            );

            let has_authors = computed_status != "owned";
            if has_authors {
                let authors = get_top_authors(repo_root, TOP_AUTHORS, &exclude)?;
                let seq = authors.into_iter().map(Value::String).collect();
                m.insert(Value::String("authors".into()), Value::Sequence(seq));
            }

            (computed_status.to_string(), m, has_authors)
        }
    };

    if let Some(filters) = filter_set
        && !filters.contains(&status.to_lowercase())
    {
        return Ok(None);
    }

    Ok(Some((slug.clone(), status, mapping)))
}

/// Runs `git shortlog -s -n --all --no-merges` and returns up to `limit` authors,
/// filtering out any whose full name appears in `exclude`.
fn get_top_authors(repo: &Path, limit: usize, exclude: &BTreeSet<String>) -> Result<Vec<String>> {
    let output = common::git::output(&["shortlog", "-s", "-n", "--all", "--no-merges"], Some(repo), None)
        .context("git shortlog failed")?;
    if !output.status.success() {
        return Ok(Vec::new());
    }
    let text = output.stdout;
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

/// Loads and parses `.github/CODEOWNERS`, classifying Missing, Empty, or Present(entries).
fn load_ownership(root: &Path) -> Result<Ownership> {
    let codeowners = root.join(".github").join("CODEOWNERS");
    if !codeowners.exists() {
        return Ok(Ownership::Missing);
    }

    let content =
        fs::read_to_string(&codeowners).wrap_err_with(|| format!("Failed to read {}", codeowners.display()))?;
    Ok(parse_codeowners(&content))
}

/// Parse CODEOWNERS text into [`Ownership`]. Shared by the local-file path
/// ([`load_ownership`]) and the remote GitHub-API path ([`fetch_remote_codeowners`])
/// so both classify identically.
fn parse_codeowners(content: &str) -> Ownership {
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

    if entries.is_empty() { Ownership::Empty } else { Ownership::Present(entries) }
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
fn determine_unowned_paths(entries: &BTreeMap<String, Vec<String>>, code_files: &[PathBuf]) -> BTreeSet<String> {
    let mut unowned = BTreeSet::new();
    for rel in code_files {
        let s = format!("/{}", rel.to_string_lossy());
        let covered = entries.keys().any(|pat| s.starts_with(pat));
        if !covered {
            let comps: Vec<&str> = s.split('/').filter(|c| !c.is_empty()).collect();
            let dir = if comps.len() <= 1 { "/".to_string() } else { format!("/{}/", comps[0]) };
            unowned.insert(dir);
        }
    }
    unowned
}

/// Builds the `serde_yaml::Mapping` for a repo:
/// each path → owner(s) or `"UNOWNED"`, in the desired order.
fn build_repo_mapping(entries: BTreeMap<String, Vec<String>>, unowned: BTreeSet<String>) -> Mapping {
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
            "owned" => 2,
            _ => 3,
        }
    }

    refs.sort_by(|a, b| rank(&a.status).cmp(&rank(&b.status)).then_with(|| a.slug.cmp(&b.slug)));

    refs
}

/// Simplified: color + status on left, two spaces, then slug.
fn print_simplified(entries: &[&Repo]) {
    let width = "unowned".len();

    for r in entries {
        let colored = match r.status.as_str() {
            "owned" => r.status.green().bold(),
            "partial" => r.status.yellow().bold(),
            "unowned" => r.status.red().bold(),
            other => other.normal(),
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
            "owned" => r.status.green().bold(),
            "partial" => r.status.yellow().bold(),
            "unowned" => r.status.red().bold(),
            other => other.normal(),
        };

        println!("{} {}:", colored, r.slug);

        match &r.value {
            Value::String(s) => {
                println!("  {}", s);
            }
            Value::Mapping(m) => {
                match m.get(Value::String("paths".into())) {
                    // A path -> owners mapping (owned/partial repos).
                    Some(Value::Mapping(paths)) => {
                        println!("  paths:");
                        for (p, owners) in paths {
                            let path = p.as_str().unwrap_or_default();
                            match owners {
                                Value::Sequence(seq) => {
                                    let list: Vec<&str> = seq.iter().filter_map(Value::as_str).collect();
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
                    // A marker string (MISSING_CODEOWNERS / EMPTY_CODEOWNERS).
                    Some(Value::String(marker)) => {
                        println!("  paths: {}", marker);
                    }
                    _ => {}
                }
                if let Some(Value::Sequence(authors)) = m.get(Value::String("authors".into())) {
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

/// Heuristic: treat certain extensions and filenames as “code”.
fn is_code_file(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name == "Dockerfile" || name == "Makefile" {
            return true;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()).map(|s| s.to_lowercase()) {
            return matches!(
                ext.as_str(),
                "py" | "js" | "jsx" | "ts" | "tsx" | "css" | "html" | "tf" | "yaml" | "yml" | "toml" | "tpl"
            );
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Remote mode: scan CODEOWNERS via the GitHub API (no local clone required).
// Remote mode cannot detect unowned PATHS (there is no local file tree to walk),
// so it reports owned vs missing/empty CODEOWNERS only.
// ---------------------------------------------------------------------------

/// Scan CODEOWNERS for every repo in each org via the GitHub API, returning the
/// same `Repo` rows as local mode (optionally filtered by `--only`).
fn scan_remote(orgs: &[String], filter_set: &Option<BTreeSet<String>>) -> Result<Vec<Repo>> {
    debug!("scan_remote: orgs={:?}", orgs);
    let token = github_token()?;
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static("ls-owners"));
    headers.insert(AUTHORIZATION, HeaderValue::from_str(&format!("token {}", token))?);
    let client = Client::builder()
        .default_headers(headers)
        .build()
        .context("failed to build HTTP client")?;

    let mut results = Vec::new();
    for owner in orgs {
        let slugs = list_repos(owner, &client).wrap_err_with(|| format!("listing repos for '{}'", owner))?;
        debug!("scan_remote: owner={} repo_count={}", owner, slugs.len());
        let exclude = match read_ex_employees(owner) {
            Ok(set) => set,
            Err(err) => {
                warn!("scan_remote: could not read ex-employees for {}: {}", owner, err);
                BTreeSet::new()
            }
        };
        for slug in slugs {
            match fetch_remote_codeowners(&slug, &client) {
                Ok(ownership) => results.push(build_remote_repo(&slug, ownership, &exclude)),
                Err(err) => warn!("scan_remote: skipping {}: {}", slug, err),
            }
        }
    }

    if let Some(filter) = filter_set {
        results.retain(|r| filter.contains(&r.status));
    }
    debug!("scan_remote: produced {} repo row(s)", results.len());
    Ok(results)
}

/// The GitHub token from `GITHUB_TOKEN` or `GH_TOKEN`.
fn github_token() -> Result<String> {
    debug!("github_token");
    env::var("GITHUB_TOKEN")
        .or_else(|_| env::var("GH_TOKEN"))
        .map_err(|_| eyre!("GitHub token missing; set GITHUB_TOKEN or GH_TOKEN"))
}

/// List every repo slug (`owner/name`) for `name`, which may be a GitHub
/// organization OR a user account: try the org endpoint first, fall back to the
/// user endpoint on 404 (so `--org scottidler` works as well as `--org cli`).
fn list_repos(name: &str, client: &Client) -> Result<Vec<String>> {
    debug!("list_repos: name={}", name);
    for kind in ["orgs", "users"] {
        if let Some(repos) = list_repos_for_kind(name, kind, client)? {
            debug!("list_repos: name={} kind={} found={}", name, kind, repos.len());
            return Ok(repos);
        }
    }
    bail!(
        "no GitHub organization or user named '{}' (both endpoints returned 404)",
        name
    )
}

/// Fetch all repo slugs under `/{kind}/{name}/repos` (kind = "orgs" | "users"),
/// following pagination. Returns `None` when the account does not exist (404),
/// so the caller can try the other kind.
fn list_repos_for_kind(name: &str, kind: &str, client: &Client) -> Result<Option<Vec<String>>> {
    let mut page = 1;
    let mut all = Vec::new();
    loop {
        let url = format!(
            "https://api.github.com/{}/{}/repos?per_page=100&page={}",
            kind, name, page
        );
        let resp = client.get(&url).send()?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let repos: Vec<JsonValue> = resp.error_for_status()?.json()?;
        if repos.is_empty() {
            break;
        }
        let count = repos.len();
        for repo in &repos {
            if let Some(full) = repo.get("full_name").and_then(JsonValue::as_str) {
                all.push(full.to_string());
            }
        }
        if count < 100 {
            break;
        }
        page += 1;
    }
    Ok(Some(all))
}

/// Fetch and classify `.github/CODEOWNERS` for `slug` via the contents API.
fn fetch_remote_codeowners(slug: &str, client: &Client) -> Result<Ownership> {
    debug!("fetch_remote_codeowners: slug={}", slug);
    let url = format!("https://api.github.com/repos/{}/contents/.github/CODEOWNERS", slug);
    let resp = client.get(&url).send()?;
    match resp.status() {
        reqwest::StatusCode::NOT_FOUND => Ok(Ownership::Missing),
        reqwest::StatusCode::OK => {
            let json: JsonValue = resp.json()?;
            let content_b64 = json
                .get("content")
                .and_then(JsonValue::as_str)
                .ok_or_else(|| eyre!("no content field in GitHub response for {}", slug))?;
            let decoded = STANDARD.decode(content_b64.replace('\n', ""))?;
            let text = String::from_utf8(decoded).wrap_err_with(|| format!("CODEOWNERS for {} is not UTF-8", slug))?;
            Ok(parse_codeowners(&text))
        }
        other => bail!("GitHub returned {} for {}", other, slug),
    }
}

/// Build a `Repo` row from remotely-fetched ownership, excluding ex-employees.
/// No unowned-path detection (no local tree), so an empty unowned set is used.
fn build_remote_repo(slug: &str, ownership: Ownership, exclude: &BTreeSet<String>) -> Repo {
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
            m.insert(Value::String("paths".into()), Value::String("EMPTY_CODEOWNERS".into()));
            ("unowned".to_string(), m)
        }
        Ownership::Present(mut entries) => {
            for owners in entries.values_mut() {
                owners.retain(|o| !exclude.contains(o));
            }
            let mut m = Mapping::new();
            m.insert(
                Value::String("paths".into()),
                Value::Mapping(build_repo_mapping(entries, BTreeSet::new())),
            );
            ("owned".to_string(), m)
        }
    };
    Repo {
        slug: slug.to_string(),
        status,
        value: Value::Mapping(mapping),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;
    use tempfile::TempDir;

    #[test]
    fn test_parse_codeowners_shared() {
        assert!(matches!(parse_codeowners(""), Ownership::Empty));
        assert!(matches!(parse_codeowners("# only a comment\n"), Ownership::Empty));
        match parse_codeowners("* @alice @bob\n/docs/ @docs\n") {
            Ownership::Present(e) => {
                assert_eq!(e.get("/").unwrap(), &vec!["alice".to_string(), "bob".to_string()]);
                assert_eq!(e.get("/docs/").unwrap(), &vec!["docs".to_string()]);
            }
            _ => panic!("expected Present"),
        }
    }

    #[test]
    fn test_build_remote_repo_statuses_and_ex_employee_filter() {
        let empty_ex = BTreeSet::new();
        assert_eq!(
            build_remote_repo("o/r", Ownership::Missing, &empty_ex).status,
            "unowned"
        );
        assert_eq!(build_remote_repo("o/r", Ownership::Empty, &empty_ex).status, "unowned");

        let mut entries = BTreeMap::new();
        entries.insert("/".to_string(), vec!["alice".to_string(), "exguy".to_string()]);
        let exclude: BTreeSet<String> = ["exguy".to_string()].into_iter().collect();
        let repo = build_remote_repo("o/r", Ownership::Present(entries), &exclude);
        assert_eq!(repo.status, "owned");
        let yaml = serde_yaml::to_string(&repo.value).unwrap();
        assert!(!yaml.contains("exguy"), "ex-employee must be filtered out: {yaml}");
        assert!(yaml.contains("alice"), "remaining owner must be present: {yaml}");
    }

    #[test]
    fn test_only_does_not_swallow_trailing_path() {
        // `--only` is a repeatable single-value flag (ArgAction::Append), so a
        // trailing path is NOT consumed as another filter value.
        let cli = Cli::try_parse_from(["ls-owners", "--only", "unowned", "/some/path"]).unwrap();
        assert_eq!(cli.only, vec!["unowned"]);
        assert_eq!(cli.paths, vec!["/some/path"]);
    }

    #[test]
    fn test_only_is_repeatable() {
        let cli = Cli::try_parse_from(["ls-owners", "--only", "owned", "--only", "unowned"]).unwrap();
        assert_eq!(cli.only, vec!["owned", "unowned"]);
    }

    fn create_test_repo_with_codeowners(
        temp_dir: &TempDir,
        repo_name: &str,
        codeowners_content: Option<&str>,
    ) -> std::path::PathBuf {
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
            .args(["remote", "add", "origin", "git@github.com:testorg/testrepo.git"])
            .output()
            .unwrap();

        // Create .github directory and CODEOWNERS if content provided
        if let Some(content) = codeowners_content {
            fs::create_dir_all(repo_path.join(".github")).unwrap();
            fs::write(repo_path.join(".github/CODEOWNERS"), content).unwrap();
        }

        repo_path
    }

    #[test]
    fn test_repo_discovery_integration() {
        let temp_dir = TempDir::new().unwrap();
        let _repo1 = create_test_repo_with_codeowners(&temp_dir, "repo1", None);
        let _repo2 = create_test_repo_with_codeowners(&temp_dir, "repo2", Some("* @owner1"));

        let discovery = RepoDiscovery::new(vec![temp_dir.path().to_string_lossy().to_string()]);
        let repos = discovery.discover().unwrap();

        assert_eq!(repos.len(), 2);
        assert!(repos.iter().any(|r| r.path.file_name().unwrap() == "repo1"));
        assert!(repos.iter().any(|r| r.path.file_name().unwrap() == "repo2"));
        assert!(repos.iter().all(|r| r.slug == "testorg/testrepo"));
    }

    #[test]
    fn test_try_process_repo_missing_codeowners() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = create_test_repo_with_codeowners(&temp_dir, "test_repo", None);

        let repo_info = common::repo::RepoInfo::new(repo_path, "testorg/testrepo".to_string());
        let result = try_process_repo(&repo_info, &None).unwrap();

        assert!(result.is_some());
        let (slug, status, _mapping) = result.unwrap();
        assert_eq!(slug, "testorg/testrepo");
        assert_eq!(status, "unowned");
    }

    #[test]
    fn test_try_process_repo_with_codeowners() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = create_test_repo_with_codeowners(&temp_dir, "test_repo", Some("* @owner1\n/docs/ @docs-team"));

        let repo_info = common::repo::RepoInfo::new(repo_path, "testorg/testrepo".to_string());
        let result = try_process_repo(&repo_info, &None).unwrap();

        assert!(result.is_some());
        let (slug, status, _mapping) = result.unwrap();
        assert_eq!(slug, "testorg/testrepo");
        // Status depends on whether there are unowned files, but should not be "unowned"
        assert_ne!(status, "unowned");
    }

    #[test]
    fn test_try_process_repo_with_filter() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = create_test_repo_with_codeowners(&temp_dir, "test_repo", None);

        let repo_info = common::repo::RepoInfo::new(repo_path, "testorg/testrepo".to_string());
        let filter_set = Some(["owned"].iter().map(|s| s.to_string()).collect());
        let result = try_process_repo(&repo_info, &filter_set).unwrap();

        // Should return None because repo is "unowned" but filter only wants "owned"
        assert!(result.is_none());
    }

    #[test]
    fn test_read_ex_employees() {
        // Test that the function handles missing config gracefully
        let result = read_ex_employees("nonexistent-org").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_is_code_file() {
        assert!(is_code_file(std::path::Path::new("test.py")));
        assert!(is_code_file(std::path::Path::new("test.js")));
        assert!(is_code_file(std::path::Path::new("test.ts")));
        assert!(is_code_file(std::path::Path::new("Dockerfile")));
        assert!(is_code_file(std::path::Path::new("Makefile")));
        assert!(!is_code_file(std::path::Path::new("test.txt")));
        assert!(!is_code_file(std::path::Path::new("README.md")));
    }
}
