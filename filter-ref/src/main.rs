use clap::Parser;
use eyre::{Result, eyre, WrapErr};
use git2::Repository;
use chrono::{Local, Duration, Utc, TimeZone};
use log::{info, debug};

mod built_info {
    include!(concat!(env!("OUT_DIR"), "/git_describe.rs"));
}

#[derive(Parser, Debug)]
#[command(name = "rmrf", about = "tool for staging rmrf-ing or bkup-ing files")]
#[command(version = built_info::GIT_DESCRIBE)]
#[command(author = "Scott A. Idler <scott.a.idler@gmail.com>")]
#[command(arg_required_else_help = true)]
struct Args {
    #[clap(short = 'd', long)]
    show_date: bool,
    #[clap(short = 'a', long)]
    show_author: bool,
    #[clap(short = 's', long, value_parser = parse_span, default_value = "6m")]
    span: (Option<Duration>, Duration),
    #[clap(value_parser)]
    ref_: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    env_logger::init();

    info!("Parsed Arguments: {:?}", args);
    let repo = Repository::discover(".")?;
    debug!("Repository discovered");

    test_ref(&repo, &args.ref_, args.show_date, args.show_author, args.span)?;
    Ok(())
}

fn test_ref(repo: &Repository, ref_: &str, show_date: bool, show_author: bool, span: (Option<Duration>, Duration)) -> Result<()> {
    let obj = repo.revparse_single(ref_).wrap_err("Failed to parse ref")?;
    let commit = obj.peel_to_commit().wrap_err("Failed to peel object to commit")?;
    let author = commit.author();
    let author_name = author.name().ok_or_else(|| eyre!("Author name not found"))?;
    let commit_time = Utc.timestamp_opt(commit.time().seconds(), 0).single().ok_or_else(|| eyre!("Invalid timestamp"))?;
    let now = Local::now();

    debug!("Commit Time: {}", commit_time);
    debug!("Current Time: {}", now);

    let (_, until) = span;
    let since_date = now - until; // Calculate 'since' as 'now - period defined by `until`
    let until_date = now; // End time is the current time

    info!("Checking between {} and {}", since_date, until_date);

    if since_date < commit_time && commit_time < until_date {
        if show_date {
            println!("{} ", commit_time);
        }
        println!("{} ", ref_);
        if show_author {
            println!("{} ", author_name);
        }
    } else {
        debug!("No output: commit date not within the specified range.");
    }
    Ok(())
}

fn parse_span(s: &str) -> Result<(Option<Duration>, Duration)> {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => Ok((Some(parse_duration(parts[0])?), parse_duration(parts[1])?)),
        1 => Ok((None, parse_duration(parts[0])?)),
        _ => Err(eyre!("Invalid span format")),
    }
}

fn parse_duration(s: &str) -> Result<Duration> {
    let len = s.len();
    let num: i64 = s[..len-1].parse()?;
    match &s[len-1..] {
        "y" => Ok(Duration::weeks(num * 52)), // Approximation
        "m" => Ok(Duration::weeks(num * 4)),  // Approximation
        "w" => Ok(Duration::weeks(num)),
        "d" => Ok(Duration::days(num)),
        _ => Err(eyre!("Invalid time unit")),
    }
}
