use super::*;
use serde_json::json;

#[test]
fn test_parse_visibility_public() {
    let repo = json!({"visibility": "public"});
    assert_eq!(parse_visibility(&repo), Visibility::Public);
}

#[test]
fn test_parse_visibility_internal() {
    let repo = json!({"visibility": "internal"});
    assert_eq!(parse_visibility(&repo), Visibility::Internal);
}

#[test]
fn test_parse_visibility_private() {
    let repo = json!({"visibility": "private"});
    assert_eq!(parse_visibility(&repo), Visibility::Private);
}

#[test]
fn test_parse_visibility_falls_back_to_private_bool_true() {
    let repo = json!({"private": true});
    assert_eq!(parse_visibility(&repo), Visibility::Private);
}

#[test]
fn test_parse_visibility_falls_back_to_private_bool_false() {
    let repo = json!({"private": false});
    assert_eq!(parse_visibility(&repo), Visibility::Public);
}

#[test]
fn test_parse_visibility_missing_fields_defaults_public() {
    let repo = json!({});
    assert_eq!(parse_visibility(&repo), Visibility::Public);
}

#[test]
fn test_parse_visibility_unknown_string_falls_back_to_private_bool() {
    let repo = json!({"visibility": "bogus", "private": true});
    assert_eq!(parse_visibility(&repo), Visibility::Private);
}

#[test]
fn test_visibility_display() {
    assert_eq!(Visibility::Public.to_string(), "public");
    assert_eq!(Visibility::Internal.to_string(), "internal");
    assert_eq!(Visibility::Private.to_string(), "private");
}
