use super::*;

/// A `Cli` with everything defaulted; tests override the fields they exercise.
fn cli() -> Cli {
    Cli {
        log_level: log::LevelFilter::Info,
        branch: None,
        list: false,
        prune: false,
        yes: false,
        default_branch: None,
    }
}

#[test]
fn test_no_branch_is_pick() {
    assert_eq!(Config::try_from(cli()).unwrap().op, Op::Pick);
}

#[test]
fn test_list_flag_is_list() {
    let config = Config::try_from(Cli { list: true, ..cli() }).unwrap();
    assert_eq!(config.op, Op::List);
}

#[test]
fn test_prune_flag_is_prune() {
    let config = Config::try_from(Cli {
        prune: true,
        yes: true,
        ..cli()
    })
    .unwrap();
    assert_eq!(config.op, Op::Prune);
    assert!(config.assume_yes);
}

#[test]
fn test_branch_is_switch() {
    let config = Config::try_from(Cli {
        branch: Some("feature/auth".to_string()),
        default_branch: Some("main".to_string()),
        ..cli()
    })
    .unwrap();
    assert_eq!(config.op, Op::Switch("feature/auth".to_string()));
    assert_eq!(config.default_branch.as_deref(), Some("main"));
}

#[test]
fn test_list_with_branch_is_error() {
    let res = Config::try_from(Cli {
        list: true,
        branch: Some("feature".to_string()),
        ..cli()
    });
    assert!(res.is_err(), "--list + branch should be rejected");
}

#[test]
fn test_list_and_prune_are_mutually_exclusive() {
    let res = Config::try_from(Cli {
        list: true,
        prune: true,
        ..cli()
    });
    assert!(res.is_err(), "--list + --prune should be rejected");
}

// ---- acquisition verbs (init / migrate / flatten) -----------------------
// (Op, Config, Cli, InitCli/MigrateCli/FlattenCli, RepoSpec all arrive via
//  `use super::*`; only clap's `Parser` trait needs an explicit import.)

use clap::Parser;

#[test]
fn test_init_cli_yields_init_op() {
    let cli = InitCli::try_parse_from(["worktree init", "myorg/myrepo"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(
        config.op,
        Op::Init(RepoSpec {
            org: "myorg".to_string(),
            repo: "myrepo".to_string(),
        })
    );
    // Defaults mirror clone's bare-acquisition inputs.
    assert_eq!(config.clonepath, std::path::PathBuf::from("."));
}

#[test]
fn test_init_cli_rejects_bad_spec() {
    let cli = InitCli::try_parse_from(["worktree init", ""]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("Failed to parse repository specification"),
        "an unparseable spec must fail loudly; got: {err}"
    );
}

#[test]
fn test_migrate_cli_with_spec() {
    let cli = MigrateCli::try_parse_from(["worktree migrate", "org/repo"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(
        config.op,
        Op::Migrate(Some(RepoSpec {
            org: "org".to_string(),
            repo: "repo".to_string(),
        }))
    );
    assert!(!config.dry_run);
}

#[test]
fn test_migrate_cli_without_spec_is_none() {
    let cli = MigrateCli::try_parse_from(["worktree migrate"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Migrate(None));
}

#[test]
fn test_flatten_cli_dry_run() {
    let cli = FlattenCli::try_parse_from(["worktree flatten", "--dry-run", "org/repo"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(
        config.op,
        Op::Flatten(Some(RepoSpec {
            org: "org".to_string(),
            repo: "repo".to_string(),
        }))
    );
    assert!(config.dry_run, "--dry-run must set dry_run");
}

#[test]
fn test_flatten_cli_without_spec_is_none() {
    let cli = FlattenCli::try_parse_from(["worktree flatten"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Flatten(None));
}
