use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
pub struct LanguageConfig {
    pub skip_dirs: Vec<String>,
    pub language_markers: Vec<MarkerEntry>,
    pub extension_markers: Vec<ExtensionEntry>,
    pub extensions: Vec<ExtensionEntry>,
}

#[derive(Debug, Deserialize)]
pub struct MarkerEntry {
    pub file: String,
    pub language: String,
}

#[derive(Debug, Deserialize)]
pub struct ExtensionEntry {
    pub ext: String,
    pub language: String,
}

static CONFIG: LazyLock<LanguageConfig> = LazyLock::new(|| {
    let yaml = include_str!("../config/languages.yml");
    serde_yaml::from_str(yaml).expect("failed to parse embedded languages.yml")
});

/// Detect the primary language of a repo at the given path.
///
/// Algorithm:
/// 1. Check language_markers: for each entry, if repo_path.join(entry.file).exists(),
///    return Some(entry.language). First match wins - order encodes priority.
/// 2. Check extension_markers: read_dir(repo_path), for each file check if its
///    extension matches any entry. First match wins.
/// 3. Fallback: WalkDir with max_depth(3), skip skip_dirs, count file extensions
///    using extensions config. Return the language with the highest count, or None.
pub fn detect_language(repo_path: &Path) -> Option<String> {
    // Step 1: exact-name marker files
    for marker in &CONFIG.language_markers {
        if repo_path.join(&marker.file).exists() {
            return Some(marker.language.clone());
        }
    }

    // Step 2: extension-based marker files in repo root
    if let Ok(entries) = fs::read_dir(repo_path) {
        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                for marker in &CONFIG.extension_markers {
                    if ext.eq_ignore_ascii_case(&marker.ext) {
                        return Some(marker.language.clone());
                    }
                }
            }
        }
    }

    // Step 3: fallback extension counting with shallow walk
    let mut counts: HashMap<&str, usize> = HashMap::new();

    for entry in WalkDir::new(repo_path)
        .max_depth(3)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir()
                && let Some(name) = e.file_name().to_str()
            {
                return !CONFIG.skip_dirs.iter().any(|d| d == name);
            }
            true
        })
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
            for mapping in &CONFIG.extensions {
                if ext.eq_ignore_ascii_case(&mapping.ext) {
                    *counts.entry(&mapping.language).or_insert(0) += 1;
                    break;
                }
            }
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(lang, _)| lang.to_string())
}

/// Check if a repo matches any of the given language filters.
/// Comparison is case-insensitive. Returns false if detected is None or filters is empty.
pub fn matches_language(detected: Option<&str>, filters: &[String]) -> bool {
    match detected {
        Some(lang) => filters.iter().any(|f| f.eq_ignore_ascii_case(lang)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::TempDir;

    #[test]
    fn test_config_loads() {
        assert!(!CONFIG.language_markers.is_empty());
        assert!(!CONFIG.extension_markers.is_empty());
        assert!(!CONFIG.extensions.is_empty());
        assert!(!CONFIG.skip_dirs.is_empty());
    }

    #[test]
    fn test_detect_language_rust_marker() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("Cargo.toml")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("Rust".to_string()));
    }

    #[test]
    fn test_detect_language_go_marker() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("go.mod")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("Go".to_string()));
    }

    #[test]
    fn test_detect_language_python_marker() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("pyproject.toml")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("Python".to_string()));
    }

    #[test]
    fn test_detect_language_typescript_before_javascript() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("tsconfig.json")).unwrap();
        File::create(dir.path().join("package.json")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("TypeScript".to_string()));
    }

    #[test]
    fn test_detect_language_extension_marker_csproj() {
        let dir = TempDir::new().unwrap();
        File::create(dir.path().join("MyApp.csproj")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("C#".to_string()));
    }

    #[test]
    fn test_detect_language_fallback_by_extension() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir(&src).unwrap();
        File::create(src.join("main.py")).unwrap();
        File::create(src.join("utils.py")).unwrap();
        File::create(src.join("helpers.py")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("Python".to_string()));
    }

    #[test]
    fn test_detect_language_fallback_dominant_wins() {
        let dir = TempDir::new().unwrap();
        // 3 Rust files vs 1 Python file
        File::create(dir.path().join("main.rs")).unwrap();
        File::create(dir.path().join("lib.rs")).unwrap();
        File::create(dir.path().join("util.rs")).unwrap();
        File::create(dir.path().join("script.py")).unwrap();
        assert_eq!(detect_language(dir.path()), Some("Rust".to_string()));
    }

    #[test]
    fn test_detect_language_empty_dir() {
        let dir = TempDir::new().unwrap();
        assert_eq!(detect_language(dir.path()), None);
    }

    #[test]
    fn test_matches_language_case_insensitive() {
        assert!(matches_language(Some("Rust"), &["rust".to_string()]));
        assert!(matches_language(Some("Rust"), &["RUST".to_string()]));
        assert!(matches_language(Some("rust"), &["Rust".to_string()]));
    }

    #[test]
    fn test_matches_language_multiple_filters() {
        let filters = vec!["rust".to_string(), "python".to_string()];
        assert!(matches_language(Some("Rust"), &filters));
        assert!(matches_language(Some("Python"), &filters));
        assert!(!matches_language(Some("Go"), &filters));
    }

    #[test]
    fn test_matches_language_none() {
        assert!(!matches_language(None, &["rust".to_string()]));
    }

    #[test]
    fn test_matches_language_empty_filters() {
        assert!(!matches_language(Some("Rust"), &[]));
    }
}
