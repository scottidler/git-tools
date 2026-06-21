# Implementation Notes: Consolidate git-tools onto Shared `common` Infrastructure

Running, append-only record of how the implementation interprets or diverges
from `2026-06-21-common-shared-infra.md`.

## Phase 1: Remove the vestigial `filter-ref` crate

### Design decisions
- Corrected `.github/workflows/binary-release.yml:50` project list to the real
  binary set (`clone reposlug ls-git-repos ls-github-repos ls-owners
  ls-stale-branches ls-stale-prs`) — the old list read `stale-branches` (wrong
  name) and `filter-ref` (now deleted) and omitted several tools, exactly as the
  doc called out.
- Also removed the stale `filter-ref/` entry from `CLAUDE.md`'s workspace
  structure list (not explicitly named in the doc, but it is a dangling
  reference to a deleted crate).

### Deviations
- None.

### Tradeoffs
- Used `git rm` (not `rkvr`) per the doc: version control is the recovery
  mechanism for a tracked crate.

### Open questions
- None.

## Phase 2: Add the primitives to `common`

### Design decisions
- `common::git::parse_git_url` (`common/src/git/url_parser.rs`) was re-expressed
  as a thin shim over `parse_repospec` (the doc's "keep as a thin shim
  temporarily"); it now transparently gains host-agnostic parsing. All 8 of its
  original tests still pass unchanged.
- Kept the file name `url_parser.rs` (pre-existing) rather than renaming to the
  architecture-diagram's `url.rs` — the rename is not in the Phase 2 plan and
  would churn `AGENTS.md` references; left for a future mechanical pass.
- Inline `#[cfg(test)] mod tests` blocks were used for the new modules
  (`spec.rs`, `run.rs`, `log.rs`) to match the existing workspace convention
  (every file here uses inline blocks) and the doc's "move tests verbatim"
  instruction, rather than the global rust rule's extract-to-`tests.rs` form.
  Extracting all inline test blocks is a separate tree-wide pass.
- `default_branch` (`common/src/repo/info.rs`) reads the bare repo's
  `symbolic-ref --short HEAD` first (a `git clone --bare` pins HEAD to the remote
  default), falling back to `refs/remotes/origin/HEAD`. Never hardcodes `main`,
  per the worktree doc.
- Bare-container recognition is filesystem-based: `<dir>/.bare` is a directory
  (`is_bare_container`, `common/src/repo/discovery.rs`). Checked before the flat
  `.git` check in `classify` because a container also carries a `.git` pointer
  file.
- `RepoInfo` git calls (`from_path`, the bare helpers) were routed through the
  new `common::git::output` runner now (common dogfoods its own runner), rather
  than leaving the ad-hoc `Command::new("git")` for Phase 3.
- Discovery's hand-unrolled 2-level scan was replaced by a single recursive
  `scan` with a `remaining`-levels budget so `max_depth` (default `Some(2)`,
  `None` = unbounded) is honored uniformly; depth-2 behavior is identical to the
  old code (levels 0,1,2 checked, descent stops at any recognized repo).
- Edition bump to 2024 turned on `let`-chains, so clippy flagged two pre-/new
  collapsible-if sites (`language.rs`, `info.rs::container_slug`); collapsed them
  into `&&`/`let` chains.

### Deviations
- None. All additions are additive; flat-clone discovery behavior is unchanged
  (verified by the retained smart-matching tests).

### Tradeoffs
- Per-worktree enumeration is exposed as an opt-in builder
  (`RepoDiscovery::with_per_worktree(true)`) rather than a separate method, so
  the default `discover()` stays one-row-per-logical-repo. Covered by a real
  fixture test now (vs. shipping untested-but-public surface for the worktree
  doc to exercise later).
- The `run` runner uses `Command::output()` (buffers both pipes) instead of
  streaming. For these short-lived list/clone CLIs the buffering is negligible
  and it matches the old silent behavior while capturing stderr for errors.

### Open questions
- None.

## Phase 3: Migrate the good citizens + reposlug

### Design decisions
- `reposlug` (`reposlug/src/main.rs`) now calls
  `common::git::get_repo_slug_from_path` and dropped `git2`, `regex`, `url`, and
  its own `parse_git_url`. Its single unit test moved with the logic into
  `common::git::spec`. The helper shells out to `git -C <dir> remote get-url
  origin`, which discovers the repo from the path exactly as
  `git2::Repository::discover` did.
- The `--log-level` flag is typed directly as `log::LevelFilter`
  (`default_value_t = LevelFilter::Info`). `LevelFilter`'s `FromStr` is already
  case-insensitive, satisfying the `cli.md` enum-flag rule without a bespoke
  `ValueEnum`.
- `ls-github-repos` uses **long-only `--log-level`** (no `-l`) because `-l` is
  already its `--lang` short. The doc's "-l/--log-level" is honored on the other
  four binaries where `-l` is free; this is the one forced exception.
- `ls-owners` had no logging at all; added the `log` dep, the `--log-level`
  flag, and a `common::log::init` call.
- `ls-stale-branches`'s two production git calls now route through
  `common::git::output`. The `fetch --prune` call deliberately uses `output`
  (not `run`) so a non-zero fetch exit stays tolerated, matching the prior
  `.output()` behavior that ignored the exit status.

### Deviations
- The verbose-mode "Remote URL: ..." line in `reposlug` was dropped — it
  required the `git2` remote object that no longer exists. Verbose still prints
  the directory; the resolved slug is logged at debug. Minor, behavior-adjacent.

### Tradeoffs
- `ls-stale-prs`'s production subprocess is `gh` (not git), so it keeps
  `std::process::Command`; only the logging flag changed. Test-fixture
  `Command::new("git")` calls (in `ls-stale-prs`, `ls-owners` test modules) were
  left as direct `Command` — they set up fixture state rather than being the
  duplicated production primitives the doc targets.

### Open questions
- None.

## Phase 4: Migrate `ls-git-repos` onto `RepoDiscovery`

### Design decisions
- `ls-git-repos/src/main.rs` now calls `RepoDiscovery::new(vec![path])
  .with_max_depth(None)` (unbounded, matching the old infinite-depth `WalkDir`)
  and reads `RepoInfo { slug, path }` directly. Deleted `find_git_repos`,
  `parse_git_config`, the GitHub-only `parse_git_url` regex, and the `walkdir`,
  `regex`, `rust-ini` deps.
- `--log-level` is **long-only** here too (`-l` is `--lang`).
- Dropped the `#[tokio::main]` async runtime and the `tokio` dep — nothing in
  `ls-git-repos` was ever awaited; discovery is synchronous.
- Tilde expansion via `shellexpand::tilde` is preserved before handing the path
  to `RepoDiscovery` (which does not expand `~`).

### Deviations
- Behavior is now **additive**, as the doc intends: the old code only listed
  repos whose `origin` matched a GitHub URL (non-GitHub or origin-less repos were
  silently skipped). Via `RepoInfo`'s host-agnostic slug + path fallback,
  ls-git-repos now lists every repo on disk (non-GitHub repos get a path-derived
  `org/repo` slug). This is the "gain host-agnostic parsing" upgrade, not a
  regression.

### Tradeoffs
- `discover()` prints per-repo extraction errors to stderr (its existing
  behavior) where the old `WalkDir` path used a silent `filter_map(.ok())`.
  Errors go to stderr, the slug list to stdout, so piping is unaffected.

### Open questions
- None.

## Phase 5: Migrate `clone`

### Design decisions
- `clone/src/main.rs` now imports `common::git` and calls
  `git::parse_repospec(...).to_string()` (RepoSpec -> "org/repo"), preserving the
  `repospec: String` used throughout the rest of the file. Deleted clone's own
  `parse_repospec`/`extract_org_repo_from_path` and their tests (now in common).
- The private `git()` helper was deleted; every `git`/`Command::new("git")` call
  routes through `common::git::run` (mutations) or `common::git::output` (reads
  that need stdout / a tolerated exit). The per-org SSH command is built via
  `git::ssh_command(key)` (shell-quoted, fixing the spaces-in-key-path bug).
- Added `-l/--log-level` to clone (the `-l` short is free here) wired to
  `common::log::init`; dropped `env_logger`. Logging still targets stderr, so the
  wrapper's "stdout is only the destination path" contract is intact.
- Per the doc seam, this phase only swapped clone's internal calls — it did
  **not** restructure clone into `lib.rs` + modules. That decomposition is the
  worktree doc's Phase 1.

### Deviations
- `fetch_revision_sha` (versioning mode) now reads stdout via
  `common::git::output`. The old code set `.stdout(Stdio::null())` *and* called
  `.output()`, so `output.stdout` was empty and the SHA parse could never
  succeed — a latent bug in `--versioning`. Routing through `output()` (which the
  doc explicitly designates for `ls-remote`) captures stdout correctly. This is a
  fix, not a regression; no test exercised `--versioning` against a live remote.

### Tradeoffs
- The `run`/`output` calls pass `cwd: None` wherever clone previously relied on
  the process CWD (it `set_current_dir`s into the repo before checkout/pull/clean
  and passes the full path to `clone`), so behavior is identical. The clone
  integration tests (7, against real clones) pass unchanged.

### Open questions
- None.

## Post-audit fixes (Architect + Staff Engineer implementation audits)

Two independent audits (Gemini Architect, Codex Staff Engineer) reviewed the
branch. Both confirmed behavior parity and the core migration as correct
(scan-depth parity, runner semantics, parser promotion, tolerated `fetch
--prune`, `git2` purge, edition bumps). Acted on the findings I agreed with:

### Fixed
- **`fetch_revision_sha` ignored the `output()` contract** (Staff Eng #2, the
  strongest finding). `common::git::output` treats a non-zero exit as data, so a
  failed `ls-remote` produced a generic "Could not find SHA for HEAD" and
  swallowed stderr. Added a `.status.success()` check that surfaces the captured
  stderr (`clone/src/main.rs::fetch_revision_sha`).
- **`get_repo_slug_from_path` still hand-rolled `Command::new("git")`** (both
  audits). Routed it through `common::git::output`
  (`common/src/git/url_parser.rs`) so its failure path carries stderr like every
  other call site, and dropped the now-unused `Command`/`Context` imports.
- **Missing non-GitHub `get_repo_slug_from_path` test** (Staff Eng #6, a named
  Testing-Strategy item). Added a fixture test that inits a repo with a
  `git@gitlab.com:...` origin and asserts the host-agnostic slug.

### Acknowledged, intentionally not changed
- **`parse_repospec("org/repo/extra")` now yields `org/repo`** (Staff Eng #4).
  This is the RepoSpec two-component contract (already noted in Phase 2); the
  explicit plain-input delta vs. clone's old whole-string return is recorded here
  for completeness. Aligns with the doc's intent.
- **`from_bare_container` does not existence-check the joined worktree path**
  (Staff Eng #5). No bare containers exist until the worktree doc executes, and
  `default_branch` errors if the bare HEAD won't resolve. The worktree doc's
  Phase 2 guarantees the default worktree is created; a guard belongs there.
- **`slug_from_path` `unknown/unknown` fallback** (Staff Eng #3). Pre-existing
  code on `main`, untouched by this work and out of scope for this design doc.
- **`log::init` has only an idempotence test, not the doc's "path resolution
  behind ENV_LOCK" test** (Staff Eng #6). Our `log::init` does no path
  resolution — it targets stderr by design (an explicit Phase 2 deviation), so
  the path-resolution test does not apply; the idempotence test covers the
  load-bearing `try_init` behavior.

## Phase 6: Remove duplicates, retire shims, drop git2

### Design decisions
- Removed the `git2` dependency from the workspace root `Cargo.toml` and
  `common/Cargo.toml`. A `grep -rn "git2"` over `*.rs` returns zero code hits;
  `Cargo.lock` no longer contains a `git2` entry. `filter-ref` (Phase 1) and
  `reposlug` (Phase 3) were its only users; common's was already dead.
- **`parse_git_url` is kept, not deleted** (resolving the doc's open question
  "delete if unused — proposed: delete"). It is *not* unused: `RepoInfo`'s
  `find_repo_root_and_slug` and `container_slug` (`common/src/repo/info.rs`) both
  call it as the host-agnostic slug parser. It now stands as the documented
  `Option`-returning shim over `parse_repospec`.
- Updated the now-stale `git-tools/CLAUDE.md` and `AGENTS.md` conventions
  (logging is `--log-level` not `env_logger`/`RUST_LOG`; the parser is
  host-agnostic `parse_repospec`; discovery is `max_depth`-configurable and
  bare-container aware; git calls route through `common::git::run`/`output`).

### Deviations
- None.

### Tradeoffs
- `env_logger` remains a workspace dependency, used solely by `common::log::init`
  (which wraps `env_logger::Builder` targeting stderr). No consumer crate
  references it directly anymore — `RUST_LOG` survives only in a doc comment.
- Test-fixture `Command::new("git")` calls (git init / remote add in
  `ls-stale-prs` and `ls-owners` test modules) were intentionally left as direct
  `Command`: they construct fixture state and are not the duplicated production
  primitives this consolidation targets. `gh` in `ls-stale-prs` likewise stays
  `Command` (it is not git).

### Verification
- `grep -rn git2 *.rs` → 0; `grep -c 'name = "git2"' Cargo.lock` → 0.
- All 8 remaining crates on edition 2024.
- No production `Command::new("git")`, no `env_logger::init()`, no `RUST_LOG`,
  no hand-rolled URL parser or directory walker outside `common`.
- `otto ci` green at every phase.

### Open questions
- None.
