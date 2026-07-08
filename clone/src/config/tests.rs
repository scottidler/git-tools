use super::*;

// `Layout`/`resolve_layout`/`find_ssh_key_for_org`/`clone_cfg_value` combo
// validation moved with `--bare`/`--migrate`/`--flatten`/`--dry-run` (Phase 3
// of the clone/worktree split); those flags no longer exist on `Cli`, so their
// combinability tests are gone. `worktree`'s own config tests now cover the
// relocated bare/migrate/flatten validation.

#[test]
fn test_config_requires_repospec() {
    use crate::cli::Cli;
    use clap::Parser;

    // `--verbose` alone satisfies clap's `arg_required_else_help`, so the
    // missing-repospec rejection is exercised in `Config::try_from`, not
    // clap's own "no args at all" help path.
    let cli = Cli::try_parse_from(["clone", "--verbose"]).unwrap();
    let err = Config::try_from(cli).unwrap_err();
    assert!(
        format!("{err}").contains("a repository specification"),
        "clone with no repospec should be rejected; got: {err}"
    );
}

#[test]
fn test_config_from_valid_cli_produces_clone_op() {
    use crate::cli::Cli;
    use clap::Parser;

    let cli = Cli::try_parse_from(["clone", "org/repo"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Clone);
    assert_eq!(config.spec.unwrap().to_string(), "org/repo");
    assert!(!config.versioning);
}

#[test]
fn test_config_flat_flag_is_accepted_as_no_op_alias() {
    use crate::cli::Cli;
    use clap::Parser;

    // --flat stays as a retained no-op alias for the default; it must still
    // parse and produce a normal Clone config.
    let cli = Cli::try_parse_from(["clone", "--flat", "org/repo"]).unwrap();
    let config = Config::try_from(cli).unwrap();
    assert_eq!(config.op, Op::Clone);
}
