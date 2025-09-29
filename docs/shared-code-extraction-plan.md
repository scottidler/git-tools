# Shared Code Extraction Plan

## Overview

This document outlines a phased approach to extract common repository discovery and processing functionality from `ls-owners` into a shared module that can be reused across all git tools in the workspace.

## Goals

- Extract repository discovery logic from `ls-owners` into a reusable shared module
- Maintain backward compatibility with existing `ls-owners` functionality
- Create a foundation for future tool consolidation
- Ensure all code is tested, warning-free, and properly utilized

## Phase 1: Extract Repository Discovery from ls-owners

### 1.1 Create Shared Library Structure

**Target:** Create `common` crate with repository discovery functionality

**Actions:**
- Create `common/Cargo.toml` with necessary dependencies
- Create `common/src/lib.rs` with module structure:
  ```
  common/
  ├── src/
  │   ├── lib.rs
  │   ├── repo/
  │   │   ├── mod.rs
  │   │   ├── discovery.rs
  │   │   └── info.rs
  │   └── git/
  │       ├── mod.rs
  │       └── url_parser.rs
  ```

**Dependencies to add to common:**
- `eyre` (error handling)
- `regex` (URL parsing)
- `git2` (git operations)
- `serde` (serialization for RepoInfo)

### 1.2 Extract Core Types

**Target:** Define shared data structures

**Extract from ls-owners:**
```rust
// common/src/repo/info.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub path: PathBuf,
    pub slug: String,
}

impl RepoInfo {
    pub fn new(path: PathBuf, slug: String) -> Self { ... }
    pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Self> { ... }
}
```

**Unit Tests Required:**
- `RepoInfo::new()` creates correct structure
- `RepoInfo::from_path()` discovers git repos correctly
- `RepoInfo::from_path()` handles non-git directories appropriately
- Slug parsing works for SSH and HTTPS URLs

### 1.3 Extract Repository Discovery Logic

**Target:** Move `find_repo_paths()` functionality to shared module

**Extract from ls-owners:**
```rust
// common/src/repo/discovery.rs
pub struct RepoDiscovery {
    paths: Vec<String>,
}

impl RepoDiscovery {
    pub fn new(paths: Vec<String>) -> Self { ... }

    pub fn discover(&self) -> Result<Vec<RepoInfo>> { ... }

    // Private helper methods
    fn is_git_repo<P: AsRef<Path>>(path: P) -> bool { ... }
    fn scan_directory<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>> { ... }
    fn scan_two_levels<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>> { ... }
}
```

**Logic to Extract:**
- Direct repo detection (`.git` folder exists)
- First-level subdirectory scanning
- Two-level deep scanning for `./org/<repo>` structures
- Path deduplication logic

**Unit Tests Required:**
- Single repo discovery (path with `.git`)
- Multi-repo discovery in flat structure (`./repo1/`, `./repo2/`)
- Two-level discovery (`./org/repo1/`, `./org/repo2/`)
- Mixed structure handling
- Non-existent path handling
- Empty directory handling
- Permission denied scenarios

### 1.4 Extract Git URL Parsing

**Target:** Centralize git URL parsing logic

**Extract from ls-owners:**
```rust
// common/src/git/url_parser.rs
pub fn parse_git_url(url: &str) -> Option<String> { ... }
pub fn get_repo_slug_from_path<P: AsRef<Path>>(path: P) -> Result<String> { ... }
```

**Logic to Extract:**
- `parse_slug()` function from ls-owners
- Git remote URL parsing for both SSH and HTTPS formats
- Error handling for malformed URLs

**Unit Tests Required:**
- SSH URL parsing (`git@github.com:org/repo.git`)
- HTTPS URL parsing (`https://github.com/org/repo.git`)
- URLs without `.git` suffix
- Invalid URL formats
- Empty/null URL handling

### 1.5 Update Workspace Configuration

**Target:** Add common crate to workspace

**Actions:**
- Add `common` to workspace members in root `Cargo.toml`
- Add `common = { path = "common" }` to workspace dependencies
- Update `ls-owners/Cargo.toml` to depend on `common`

### 1.6 Refactor ls-owners to Use Shared Code

**Target:** Replace extracted code with shared module usage

**Refactoring Steps:**
1. Replace `find_repo_paths()` calls with `RepoDiscovery::new(paths).discover()`
2. Replace `parse_slug()` calls with `common::git::parse_git_url()`
3. Update imports to use shared modules
4. Remove now-duplicated code from ls-owners

**Critical Requirements:**
- **No functional changes** to ls-owners behavior
- **All existing tests pass** (if any exist)
- **No dead code warnings** - every piece of extracted code must be used
- **No underscore prefixes** to silence warnings
- **No `#[allow(dead_code)]`** directives

**Integration Tests Required:**
- ls-owners produces identical output before/after refactoring
- All CLI argument combinations work identically
- Error cases behave the same way
- Performance is not significantly degraded

## Phase 2: Testing and Validation

### 2.1 Comprehensive Test Suite

**Unit Tests:**
- All extracted functions have 100% test coverage
- Edge cases are thoroughly covered
- Error conditions are tested

**Integration Tests:**
- ls-owners behavior is unchanged
- All CLI combinations work correctly
- Performance benchmarks pass

**Test Data Structure:**
```
common/
├── tests/
│   ├── fixtures/
│   │   ├── single-repo/
│   │   ├── multi-repo/
│   │   ├── nested-org/
│   │   └── mixed-structure/
│   ├── integration_tests.rs
│   └── unit_tests.rs
```

### 2.2 Dead Code Elimination Strategy

**Approach:**
1. **Identify all extracted code** - catalog every function, struct, and constant moved
2. **Ensure usage** - each piece must be actively used by ls-owners or tests
3. **Remove unused code** - only if absolutely certain it's not needed
4. **Document decisions** - why each piece of code was kept or removed

**Validation Process:**
1. `cargo check` produces no warnings
2. `cargo test` all tests pass
3. `cargo clippy` produces no warnings
4. Manual verification of ls-owners functionality

## Phase 3: Documentation and Cleanup

### 3.1 Documentation

**API Documentation:**
- All public functions have rustdoc comments
- Examples in documentation
- Module-level documentation explaining purpose

**Usage Documentation:**
- Update README.md with shared module information
- Document the extraction process
- Provide migration examples for future tools

### 3.2 Code Quality

**Standards:**
- All code follows project style guidelines
- No compiler warnings
- No clippy warnings
- Proper error handling throughout

## Success Criteria

**Phase 1 Complete When:**
- [ ] `common` crate compiles without warnings
- [ ] All unit tests pass
- [ ] `ls-owners` uses shared code exclusively
- [ ] `ls-owners` behavior is unchanged
- [ ] No dead code warnings anywhere
- [ ] Integration tests pass

**Risks and Mitigations:**

| Risk | Mitigation |
|------|------------|
| Breaking ls-owners functionality | Comprehensive integration testing before/after |
| Dead code warnings | Careful analysis of each extracted piece |
| Performance regression | Benchmark critical paths |
| Over-extraction | Start minimal, expand iteratively |

## Future Phases (Not in Scope)

- **Phase 4:** Migrate `ls-stale-prs` to use shared repo discovery
- **Phase 5:** Migrate `ls-stale-branches` to use shared repo discovery
- **Phase 6:** Extract common CLI patterns
- **Phase 7:** Extract common output formatting

## Implementation Notes

- **Incremental approach:** Each step should compile and pass tests
- **Backward compatibility:** ls-owners must work identically throughout
- **Test-driven:** Write tests before refactoring
- **Documentation-driven:** Document the extracted API as it's created

This phased approach ensures a safe, tested extraction of shared functionality while maintaining the reliability and behavior of existing tools.