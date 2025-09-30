use clap::Parser;
use git2::Repository;
use eyre::{Result, eyre};
use regex::Regex;

// Built-in version from build.rs via env!("GIT_DESCRIBE")

#[derive(Parser, Debug)]
#[command(name = "reposlug", about = "get the reposlug from the remote origin url")]
#[command(version = env!("GIT_DESCRIBE"))]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
struct Args {
    #[clap(short, long)]
    verbose: bool,
    #[clap(value_parser, help = "[default: .]")]
    directory: Option<String>, // Make this optional
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Setup logging
    env_logger::init();

    // Use the provided directory or default to "."
    let directory = args.directory.unwrap_or_else(|| String::from("."));

    if args.verbose {
        println!("Using directory: {}", directory);
    }

    // Open the repository from the specified directory
    let repo = Repository::discover(&directory)?;
    let remote = repo.find_remote("origin")?;
    let remote_url = remote.url().ok_or_else(|| eyre!("Remote 'origin' URL not found"))?;

    if args.verbose {
        println!("Remote URL: {}", remote_url);
    }

    let repo_slug = parse_git_url(remote_url)?;

    println!("{}", repo_slug);

    Ok(())
}

fn parse_git_url(url: &str) -> Result<String> {
    let re = Regex::new(
        r"(?x)
        ^(?:git|https?|ssh)://   # Match the protocol
        (?:[^@]+@)?              # Match the user authentication if present
        [^:/]+                   # Match the host (not capturing)
        [:/]                     # Match the separator after the host
        (?P<slug>[^/]+/[^/]+?)   # Capture the slug
        (?:\.git)?               # Match the .git extension, if present
        $|                       # Alternation for the next pattern
        ^git@                    # Match the git@ prefix
        [^:/]+                   # Match the host (not capturing)
        :(?P<slug_2>[^/]+/[^/]+?)  # Capture the slug
        (?:\.git)?               # Match the .git extension, if present
        $"                       // End of line
    ).map_err(|_| eyre!("Invalid regex pattern"))?;

    re.captures(url)
        .and_then(|caps| caps.name("slug").or_else(|| caps.name("slug_2")).map(|m| m.as_str().to_string()))
        .ok_or_else(|| eyre!("Failed to parse URL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_git_urls() {
        let urls = vec![
            "https://github.com/repo/slug",
            "git@github.com:repo/slug",
            "ssh://git@github.com/repo/slug",
            "git://github.com/repo/slug",
        ];

        for url in urls {
            assert_eq!(parse_git_url(url).unwrap(), "repo/slug", "URL parsing failed for: {}", url);
        }
    }
}

