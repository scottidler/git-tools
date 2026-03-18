# Design Document: Language Filtering for ls-github-repos and ls-git-repos

**Author:** Scott Idler
**Date:** 2026-03-18
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Add `--lang <language>` filtering to both `ls-github-repos` and `ls-git-repos` so users can narrow repo listings by programming language. `ls-github-repos` will use the GitHub API's existing `language` field; `ls-git-repos` will detect language locally via marker files and file extension analysis, using `rayon` parallel iteration for performance.

## Usage Examples

```bash
# List only Rust repos under a GitHub user
ls-github-repos scottidler --lang rust

# List Python and Rust repos (union)
ls-github-repos scottidler --lang rust --lang python

# List only Rust repos locally
ls-git-repos ~/repos --lang rust

# Combine with existing flags
ls-github-repos scottidler --lang rust --age
```

## Problem Statement

### Background

`ls-github-repos` and `ls-git-repos` are Rust CLI tools in the `git-tools` workspace. `ls-github-repos` lists repos under a GitHub org/user via the REST API. `ls-git-repos` discovers local Git repos by walking the filesystem. Neither tool currently supports filtering by programming language.

### Problem

When a user has dozens or hundreds of repos, they often want to find repos in a specific language (e.g., "show me all my Rust repos"). Today, this requires manual inspection or external scripting. The GitHub API already returns a `language` field per repo, but `ls-github-repos` ignores it. For local repos, no language detection exists.

### Goals

- Add `--lang <language>` flag to `ls-github-repos` that filters by GitHub's reported primary language
- Add `--lang <language>` flag to `ls-git-repos` that detects language locally and filters
- Language matching should be case-insensitive
- Support multiple `--lang` values (e.g., `--lang rust --lang python`)
- Local language detection should be parallelized via `rayon`
- Add a shared `detect_language` module in `common` for reuse

### Non-Goals

- Full linguist-style analysis (byte-counting per language, percentage breakdowns)
- Supporting `--lang` as a GitHub API query parameter (the repos endpoint doesn't support it)
- Adding language info to the output format (this is purely a filter; `--lang` narrows results, it doesn't add a language column)
- Changing the existing `common::RepoInfo` struct to carry language data

## Proposed Solution

### Overview

Two separate but parallel changes:

1. **ls-github-repos**: Extract the `language` field from the API response (already present in payload), filter repos where language matches any of the requested `--lang` values.

2. **ls-git-repos**: Add a `detect_language(path) -> Option<String>` function in `common` that checks for marker files and counts file extensions. Use `rayon::par_iter` to run detection across discovered repos in parallel. Filter repos that match any requested `--lang` value.

### Architecture

```
common/
  config/
    languages.yml   - NEW: YAML config defining all language markers and extensions
  src/
    lib.rs          - add `pub mod language;`
    language.rs     - NEW: load_config(), detect_language(), matches_language()

ls-github-repos/src/main.rs
  - Add --lang CLI arg
  - Extract repo["language"] in the fetch loop
  - Filter by language match

ls-git-repos/src/main.rs
  - Add --lang CLI arg
  - Add rayon dependency
  - After discovering repos, par_iter to detect language and filter
```

### Data Model

#### YAML config file (`common/config/languages.yml`)

All language detection data lives in a single YAML file, embedded into the binary at compile time
via `include_str!("../config/languages.yml")` and parsed once at startup using `serde_yaml`.

```yaml
# Language detection configuration for ls-git-repos.
# Embedded into the binary at compile time via include_str!.
#
# Future: could support runtime override from ~/.config/git-tools/languages.yml
# (Option C) to allow user customization without recompiling.

# Directories to skip during extension counting (performance).
skip_dirs:
  - .git
  - node_modules
  - target
  - vendor
  - dist
  - build
  - __pycache__
  - .tox
  - .venv
  - venv

# Exact-name marker files checked via repo_path.join(name).exists().
# Checked in order; first match wins - order encodes priority.
language_markers:
  - file: Cargo.toml
    language: Rust
  - file: go.mod
    language: Go
  - file: pyproject.toml
    language: Python
  - file: setup.py
    language: Python
  - file: tsconfig.json
    language: TypeScript
  - file: package.json
    language: JavaScript
  - file: pom.xml
    language: Java
  - file: build.gradle
    language: Java
  - file: build.gradle.kts
    language: Kotlin
  - file: CMakeLists.txt
    language: C++
  - file: mix.exs
    language: Elixir
  - file: Gemfile
    language: Ruby
  - file: composer.json
    language: PHP
  - file: build.zig
    language: Zig
  - file: dune-project
    language: OCaml
  - file: stack.yaml
    language: Haskell
  - file: flake.nix
    language: Nix
  - file: shell.nix
    language: Nix

# Extension-based marker files detected by scanning the repo root directory.
# Used for languages whose project files have variable names (e.g., *.csproj, *.cabal).
extension_markers:
  - ext: csproj
    language: C#
  - ext: cabal
    language: Haskell

# Maps file extensions to languages for fallback counting.
# When no marker file is found, source files are counted (max_depth: 3)
# and the language with the most files wins.
extensions:
  - ext: rs
    language: Rust
  - ext: go
    language: Go
  - ext: py
    language: Python
  - ext: js
    language: JavaScript
  - ext: ts
    language: TypeScript
  - ext: tsx
    language: TypeScript
  - ext: jsx
    language: JavaScript
  - ext: java
    language: Java
  - ext: kt
    language: Kotlin
  - ext: cs
    language: C#
  - ext: cpp
    language: C++
  - ext: cc
    language: C++
  - ext: c
    language: C
  - ext: h
    language: C
  - ext: rb
    language: Ruby
  - ext: php
    language: PHP
  - ext: ex
    language: Elixir
  - ext: exs
    language: Elixir
  - ext: zig
    language: Zig
  - ext: ml
    language: OCaml
  - ext: hs
    language: Haskell
  - ext: nix
    language: Nix
  - ext: swift
    language: Swift
  - ext: scala
    language: Scala
  - ext: dart
    language: Dart
  - ext: lua
    language: Lua
  - ext: sh
    language: Shell
  - ext: bash
    language: Shell
  - ext: zsh
    language: Shell
```

#### Rust deserialization structs (`common/src/language.rs`)

```rust
use serde::Deserialize;
use std::sync::LazyLock;

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
```

### API Design

#### `common/src/language.rs` - public API

```rust
use std::path::Path;

/// Detect the primary language of a repo at the given path.
/// Returns the detected language name (e.g., "Rust", "Python") or None.
/// Uses the embedded languages.yml config loaded via LazyLock.
///
/// Algorithm:
/// 1. Check CONFIG.language_markers: for each entry, if repo_path.join(entry.file).exists(),
///    return Some(entry.language). First match wins - order encodes priority.
/// 2. Check CONFIG.extension_markers: read_dir(repo_path), for each file check if its
///    extension matches any entry. First match wins.
/// 3. Fallback: WalkDir::new(repo_path).max_depth(3).follow_links(false), skip
///    CONFIG.skip_dirs, count file extensions using CONFIG.extensions. Return the language
///    with the highest count, or None if no matches.
pub fn detect_language(repo_path: &Path) -> Option<String> { ... }

/// Check if a repo matches any of the given language filters.
/// Comparison is case-insensitive (e.g., "rust" matches "Rust").
/// Returns false if detected is None or filters is empty.
pub fn matches_language(detected: Option<&str>, filters: &[String]) -> bool {
    match detected {
        Some(lang) => filters.iter().any(|f| f.eq_ignore_ascii_case(lang)),
        None => false,
    }
}
```

#### CLI changes for both tools

```rust
#[clap(short, long, num_args = 1..)]
lang: Vec<String>,
```

This allows: `ls-github-repos scottidler --lang rust` and `ls-github-repos scottidler --lang rust --lang python`

### Implementation Plan

#### Phase 1: `common` config and language module

1. Create `common/config/languages.yml` with all language markers, extension markers, extensions, and skip_dirs
2. Create `common/src/language.rs` with `LanguageConfig` structs, `LazyLock<LanguageConfig>` via `include_str!`, `detect_language()`, and `matches_language()`
3. Add `walkdir` and `serde_yaml` as dependencies in `common/Cargo.toml`
4. Export via `common/src/lib.rs`
5. Add unit tests for marker detection and extension counting

#### Phase 2: `ls-github-repos` changes

1. Add `--lang` CLI argument to `Cli` struct
2. In `ls_github_repos()`, extract `repo["language"]` alongside `full_name` and `created_at`
3. Change return type from `Vec<(String, String)>` to `Vec<(String, String, Option<String>)>` (name, date, language)
4. In `main()`, filter results using `matches_language()` before printing
5. Add integration test

#### Phase 3: `ls-git-repos` changes

1. Add `common` and `rayon` as dependencies in `ls-git-repos/Cargo.toml`
2. Add `--lang` CLI argument to `Cli` struct
3. Refactor `find_git_repos()` to return `Vec<(String, PathBuf)>` (slug, repo_root) instead of `Vec<String>`. The repo root is the parent of `.git/` (i.e., `config_path.parent().parent()`). This is needed so `detect_language()` can inspect the repo's files.
4. When `--lang` is non-empty: use `rayon::par_iter` over the `(slug, path)` pairs to run `detect_language()` on each path, then filter with `matches_language()`. Collect filtered slugs.
5. When `--lang` is empty: no detection, no performance change - just print slugs as before.
6. Add integration test

## Alternatives Considered

### Alternative 1: GitHub API Languages endpoint for ls-github-repos

- **Description:** Call `GET /repos/{owner}/{repo}/languages` for each repo to get detailed language breakdown
- **Pros:** More accurate; shows all languages, not just primary
- **Cons:** Requires N additional API calls (one per repo); hits rate limits quickly for large orgs; massive slowdown
- **Why not chosen:** The `language` field in the repo listing response is sufficient for filtering and is free (no extra API calls)

### Alternative 2: ls-git-repos marker files only (no extension counting)

- **Description:** Only check for marker files like `Cargo.toml`, `go.mod`, etc. - no file extension analysis
- **Pros:** Extremely fast; no directory walking beyond repo root
- **Cons:** Misses repos without standard marker files; can't detect language for repos that only have source files
- **Why not chosen:** The extension fallback provides a safety net for repos without marker files. With `max_depth: 3` and `rayon`, the performance cost is negligible.

### Alternative 3: Use tokio async tasks instead of rayon for ls-git-repos

- **Description:** Since ls-git-repos already uses tokio, use `tokio::spawn` + `JoinSet` for parallel language detection
- **Pros:** Consistent async model; no additional runtime dependency
- **Cons:** File I/O in async tasks blocks the runtime without `spawn_blocking`; rayon is purpose-built for CPU-bound parallel work; `walkdir` is synchronous
- **Why not chosen:** `rayon` is already a workspace dependency (used by `common`), is the right tool for parallel filesystem operations, and avoids async complexity for inherently synchronous work

### Alternative 4: Runtime config override (`~/.config/git-tools/languages.yml`)

- **Description:** In addition to the embedded YAML, check for a user-local override file at runtime that merges with or replaces the built-in config
- **Pros:** Users can add languages without recompiling; per-machine customization
- **Cons:** Adds runtime file I/O; config drift between machines; merge semantics need defining
- **Why not chosen (yet):** The embedded config covers 22 languages which is sufficient for now. This is a natural future enhancement if users need to add custom languages without rebuilding. The `LanguageConfig` struct and YAML format are already designed to support this - it would only require adding a config file check before falling back to the embedded default.

### Alternative 5: Add language as an output column rather than a filter

- **Description:** Instead of `--lang` as a filter, add `--show-lang` to display language alongside repo names
- **Pros:** More informative output
- **Cons:** Different feature; can be added later independently
- **Why not chosen:** The immediate need is filtering. Display enhancement is orthogonal and can be a follow-up

## Technical Considerations

### Dependencies

- `common/Cargo.toml`: Add `walkdir = "2.5.0"` and `serde_yaml = "0.9"` (walkdir already used by `ls-git-repos`, serde_yaml already used by `ls-github-repos`)
- `ls-git-repos/Cargo.toml`: Add `common = { path = "../common" }` and `rayon = "1.10.0"` (neither currently listed)
- `ls-github-repos/Cargo.toml`: Add `common = { path = "../common" }` (not currently listed)
- No new *external* crates introduced to the workspace - `walkdir`, `rayon`, and `serde_yaml` are already used by other members

### Performance

- **ls-github-repos:** Zero performance impact - language field is already in the API response payload, just not extracted. Filtering is O(n) string comparison.
- **ls-git-repos:** Language detection adds filesystem reads per repo. Marker file check is a single `exists()` call per marker (fast path). Extension counting uses `WalkDir` with `max_depth(3)` to avoid deep traversal. `rayon::par_iter` ensures detection runs across repos in parallel, utilizing all available cores. For a typical `~/repos/` with ~100 repos, detection should complete in under 1 second.
- **Optimization:** Short-circuit on marker file match - if `Cargo.toml` exists, return `"Rust"` immediately without counting extensions.

### Security

No security implications. No new network calls, no credential handling changes, no user input passed to shell commands.

### Testing Strategy

- **Unit tests** for `common::language::detect_language()`:
  - Repo with `Cargo.toml` -> `Some("Rust")`
  - Repo with `go.mod` -> `Some("Go")`
  - Repo with only `.py` files -> `Some("Python")`
  - Empty repo -> `None`
  - Repo with mixed files -> returns dominant language
- **Unit tests** for `common::language::matches_language()`:
  - Case-insensitive matching (`"rust"` matches `"Rust"`)
  - Multiple filters (any match returns true)
  - `None` detected language returns false
- **Integration tests:** Create temp directories with marker files, run detection, verify results

### Rollout Plan

1. Implement and test `common/src/language.rs`
2. Integrate into `ls-github-repos`, bump version
3. Integrate into `ls-git-repos`, bump version
4. Single PR with all changes

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Marker file misidentifies language (e.g., `package.json` in a Rust project) | Medium | Low | Marker priority order matters; primary marker (Cargo.toml) checked first. Users can also pass multiple `--lang` values. |
| Extension counting is slow for very large repos (monorepos) | Low | Medium | `max_depth(3)` limits traversal. `rayon` parallelizes across repos. Marker files short-circuit. |
| GitHub API `language` field is null for empty/new repos | Medium | Low | `matches_language()` handles `None` gracefully by returning false (repo excluded from filtered results) |
| Language name mismatch between GitHub API and local detection | Low | Medium | Normalize language names in `matches_language()` using a canonical mapping. Both sources should use the same name strings (e.g., "TypeScript" not "typescript"). |
| TypeScript project detected as JavaScript (both markers present) | Medium | Low | `tsconfig.json` is checked before `package.json` in marker order, so TS projects are correctly identified. |
| Symlinks cause infinite loops or count external files | Low | High | `WalkDir` configured with `follow_links(false)`. |

## Open Questions

- [ ] Should `--lang` support partial matches (e.g., `--lang type` matching "TypeScript")? Recommendation: no, use exact case-insensitive match.
- [ ] Should `--lang` with no repos matching print a message or just empty output? Recommendation: empty output (consistent with `grep` behavior).
- [x] ~~For `ls-git-repos`, should the walker skip common non-source directories?~~ Yes - defined in `SKIP_DIRS` constant.

## References

- GitHub REST API repos endpoint: https://docs.github.com/en/rest/repos/repos
- Existing shared code plan: `docs/shared-code-extraction-plan.md`
- `common` crate: `common/src/` (RepoInfo, RepoDiscovery, ParallelExecutor)
- `rayon` docs: https://docs.rs/rayon
