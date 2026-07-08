use super::*;

#[test]
fn test_remote_urls_constant() {
    assert_eq!(REMOTE_URLS.len(), 2);
    assert_eq!(REMOTE_URLS[0], "ssh://git@github.com");
    assert_eq!(REMOTE_URLS[1], "https://github.com");
}
