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
