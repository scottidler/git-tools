use super::*;

#[test]
fn test_parse_selection_takes_path_field() {
    assert_eq!(
        parse_selection("main\t/repos/org/repo/main").unwrap(),
        "/repos/org/repo/main"
    );
}

#[test]
fn test_parse_selection_trims_trailing_newline() {
    assert_eq!(
        parse_selection("dev\t/repos/org/repo/dev\n").unwrap(),
        "/repos/org/repo/dev"
    );
}

#[test]
fn test_parse_selection_keeps_locked_label_path() {
    // The label column may carry a " [locked]" marker; the path is still field 2.
    assert_eq!(
        parse_selection("pinned [locked]\t/repos/org/repo/pinned").unwrap(),
        "/repos/org/repo/pinned"
    );
}

#[test]
fn test_parse_selection_rejects_lineless_input() {
    assert!(parse_selection("no-tab-here").is_err());
}
