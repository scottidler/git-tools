# Design Document: Consolidate git-tools onto Shared `common` Infrastructure

**Author:** Scott A. Idler
**Date:** 2026-06-21
**Status:** Implemented
**Review Passes Completed:** 5/5 (Draft → Correctness → Clarity → Edge Cases → Excellence)

## Summary

The git-tools workspace already has a shared library crate (`common`), but
adoption is partial and inconsistent: the same jobs (git-URL→slug parsing, repo
discovery, git subprocess invocation, logging) are implemented up to four
different ways across the eight binaries, and the single most capable
implementation of each is often the one *not* in `common`. This design makes
`common` own every cross-cutting primitive, has every remaining binary consume
it, deletes one vestigial tool (`filter-ref`), and removes `git2` from the
workspace.
It is a prerequisite for the worktree-savvy `clone` work
(`2026-06-21-clone-bare-worktree.md`), which builds on the consolidated
primitives rather than adding a fifth parser or a second discovery path.

## Problem Statement

### Background

`common` exports `git` (a GitHub-only `parse_git_url`), `language`, `parallel`
(`ParallelExecutor`), and `repo` (`RepoDiscovery`, `RepoInfo`). Audit of the
workspace (2026-06-21):

| Crate | consumes `common`? | Notes |
|---|---|---|
| `ls-owners` | yes | `RepoDiscovery` + `ParallelExecutor` + `RepoInfo` |
| `ls-stale-branches` | yes | same |
| `ls-stale-prs` | yes | same |
| `ls-github-repos` | yes | only `common::language` (REST API tool) |
| `ls-git-repos` | yes | **but ignores `RepoDiscovery`** — own `WalkDir` + GitHub-regex `parse_git_config` |
| `clone` | **no** | own `parse_repospec`, own `git()` helper, own path logic |
| `reposlug` | **no** | own GitHub-only `parse_git_url` |
| `filter-ref` | **no** | neither |

### Problem

The same work is duplicated and divergent:

- **Git-URL → slug: four implementations.** `common::git::parse_git_url`
  (GitHub-only), `reposlug::parse_git_url` (GitHub-only — reposlug's *entire job*
  is slug-from-remote, reimplemented weaker), `ls-git-repos::parse_git_config`
  (Ini + GitHub regex), and `clone::parse_repospec` — the **most capable**
  (org/repo, https, ssh://, git://, scp-style, `.git` stripping, extra path
  segments, host-agnostic so GitLab/Bitbucket/enterprise work) — trapped inside
  `clone`. The best parser is the one not shared.
- **Repo discovery: two implementations.** `common::RepoDiscovery` (3 consumers)
  vs `ls-git-repos`'s private `WalkDir`, even though `ls-git-repos` links
  `common`.
- **Git subprocess: no shared runner.** `clone` has a private `git()` that nulls
  stderr; `common::repo::info`, `common::git`, `ls-owners`, and the `ls-stale-*`
  tools each hand-roll `Command::new("git")`. The absent primitive is *why*
  stderr handling is inconsistent and subprocess failures are opaque.
- **Logging: uniformly wrong.** Every binary uses `env_logger`/`RUST_LOG`,
  violating the project `--log-level` rule across the board.
- **Version: uniformly right** (all eight: `build.rs` + `GIT_DESCRIBE`), but the
  `build.rs` is copy-pasted eight times.
- **Edition: inconsistent.** `ls-github-repos` and `ls-owners` are on edition
  2024; the other seven (`common` included) are still 2021, against the workspace
  standard of 2024.
- **git2 vs shell-out: inconsistent.** Only `filter-ref` and `reposlug` use the
  native `git2`/libgit2 binding; the other six shell out to the `git` CLI.
  `common` *declares* `git2` in its Cargo.toml but never uses it (dead dep). So
  `git2` is carried for two tools, one of which is vestigial.
- **`filter-ref` is vestigial.** It self-identifies as `rmrf` (a copy-paste
  scaffold leftover: `#[command(name = "rmrf", about = "tool for staging
  rmrf-ing or bkup-ing files")]`, main.rs:10), half its CLI is dead (the `since`
  half of `--span` is parsed then discarded, main.rs:59), it has no tests/docs,
  and its function — print a single ref iff its tip is within a time span — is a
  weaker subset of `ls-stale-branches`. No standalone purpose could be
  determined; it is slated for deletion (see Phase 1).

### Goals

- One implementation per cross-cutting job, living in `common`.
- All remaining binaries (seven, after `filter-ref` is deleted) depend on
  `common` and use its primitives.
- The unified git-URL parser is host-agnostic and format-complete (clone's
  superset), so no caller loses capability and `reposlug`/`ls-git-repos` gain it.
- One git-command runner with captured stderr and contextual, typed errors.
- One `--log-level`-based logging setup; `RUST_LOG` removed everywhere.
- `RepoDiscovery` is the single discovery path (bare-container awareness is added
  here and consumed by the worktree design).
- All crates on edition 2024 (the workspace standard).
- Delete the vestigial `filter-ref` crate, and eliminate `git2` from the
  workspace entirely (every tool shells out to the `git` CLI).

### Non-Goals

- The worktree/bare-repo feature itself (separate doc). This doc only adds the
  bare-container *primitives* to `common`; the worktree behavior consumes them.
- Changing each tool's user-facing behavior beyond the logging flag and the
  strictly-additive parser capability.
- A shared `build.rs` mechanism is out of scope beyond noting it (low value).
- Reworking `ParallelExecutor` or `language` (already shared and adopted).

## Proposed Solution

### Overview

Grow `common` into the workspace's infrastructure crate with four consolidated
modules, then migrate each binary onto them, deleting the duplicates.

### Architecture

```
common/src/
  git/
    mod.rs        # re-exports
    spec.rs       # parse_repospec (promoted from clone) -> RepoSpec
    url.rs        # get_repo_slug_from_path (shells out, kept); parse_git_url re-expressed on RepoSpec
    run.rs        # git command runner: run() / output() with captured stderr
  repo/
    discovery.rs  # single RepoDiscovery; bare-container aware
    info.rs       # RepoInfo (+ optional worktree field), slug via git::url
  log.rs          # init(level): --log-level -> stderr (preserves today's behavior)
  language.rs     # unchanged
  parallel.rs     # unchanged
```

### API Design

Legend: **(moved)** = relocated from a binary into `common` verbatim;
**(new)** = newly written here; **(unchanged)** = existing `common` API kept.

```rust
// common::git::spec — the single parser (moved from clone::parse_repospec)
pub struct RepoSpec { pub org: String, pub repo: String }      // (new) Display -> "org/repo"
pub fn parse_repospec(input: &str) -> Result<RepoSpec>;        // (moved) all formats, host-agnostic; first two path components
pub fn slugify_branch(name: &str) -> String;                   // (new) lowercase-hyphenated (used by clone --worktree)

// common::git::url — slug from a remote/config (existing, shells out to git)
pub fn get_repo_slug_from_path(path: impl AsRef<Path>) -> Result<String>;  // (unchanged) reads origin from disk; reposlug's new path
// parse_git_url(url) is (unchanged for now) re-expressed over parse_repospec, then retired in Phase 6 if unused.

// common::git::run — the single git runner. BOTH take envs (e.g. GIT_SSH_COMMAND).
pub struct GitOutput { pub stdout: String, pub stderr: String, pub status: ExitStatus }
pub fn run(args: &[&str], cwd: Option<&Path>, envs: Option<&[(&str,&str)]>) -> Result<()>;                 // non-zero exit -> Err carrying captured stderr
pub fn output(args: &[&str], cwd: Option<&Path>, envs: Option<&[(&str,&str)]>) -> Result<GitOutput>;       // captures both pipes; non-zero exit is NOT an error (caller inspects .status)
// Clone's SSH->HTTPS fallback uses `run()`: it errors on non-zero AND carries
// GIT_SSH_COMMAND via `envs`, so the call site catches Err and tries the next
// remote (preserving today's `attempt_clone -> Result<bool>` behavior at
// clone/src/main.rs:382-411). `output()` is for read commands that need stdout
// (ls-remote, rev-parse, worktree list) — `envs` is present on it too because
// `ls-remote` in versioning mode hits the network and needs the SSH key.

// common::repo — unchanged signatures; discovery internals upgraded
impl RepoDiscovery { pub fn discover(&self) -> Result<Vec<RepoInfo>>; }
// RepoInfo gains: pub worktree: Option<String>  (None for flat clones / logical rows)

// common::log
pub fn init(level: log::LevelFilter, project: &str) -> Result<()>;   // --log-level -> stderr; no RUST_LOG; idempotent (try_init)
```

### Consolidation decisions

- **Parser:** `clone::parse_repospec` is a strict superset of the GitHub-only
  copies, so promoting it loses nothing and upgrades `reposlug`/`ls-git-repos`
  to host-agnostic parsing. The GitHub-only `common::git::parse_git_url` is
  re-expressed in terms of `parse_repospec` (or removed if unused after
  migration).
  - **`RepoSpec { org, repo }` is two-component by design, and faithful.**
    clone's `extract_org_repo_from_path` already takes only the first two path
    components (`parts[0]`/`parts[1]`, clone/src/main.rs:107-108) and discards
    the rest — `gitlab.com/org/team/sub/repo` maps to `org=org, repo=team`
    today. That is not a regression introduced by `RepoSpec`; it is the existing
    behavior and the only mapping consistent with the two-level
    `~/repos/<org>/<repo>` on-disk invariant. The hardest-question case resolves
    to `org`/`team` and an on-disk path of `~/repos/org/team`. (Deeper GitLab
    subgroup support is a separate, pre-existing limitation; out of scope here,
    noted so it is a known constraint rather than a silent surprise.)
  - **`reposlug` migrates to the existing shell-out helper, not a URL string.**
    `reposlug` uses `git2::Repository::discover` to read `origin` from disk
    today; `common::git::url_parser::get_repo_slug_from_path` already exists and
    shells out to do exactly that. `reposlug` calls it (dropping `git2`); map its
    error to reposlug's existing "could not parse remote" exit behavior.
- **Discovery (this doc is the sole owner of all `RepoDiscovery`/`RepoInfo`
  changes; the worktree doc only consumes them):** `RepoDiscovery` becomes the
  only path. `ls-git-repos` migrates off its `WalkDir`. `is_git_repo` accepts a
  `.git` file or directory; bare-container recognition and worktree enumeration
  (`git worktree list --porcelain`) live here. Discovery yields **one `RepoInfo`
  per logical repo** (per container for bare repos).
  - **`RepoInfo.path` contract (the load-bearing decision).** Consumers
    dereference `path` two ways: `ls-owners` as a checked-out working-tree root
    (`path/.github/CODEOWNERS`, code scan — ls-owners/src/main.rs:126,213) and
    `ls-stale-branches` as a git cwd (ls-stale-branches/src/main.rs:55,84). A
    bare container is neither a checked-out root nor (for ls-owners) usable. So
    for a bare container **`RepoInfo.path` = the canonical default-branch
    worktree** (`<container>/<default-branch>`, always present), and `slug` =
    `org/repo`. This is backward-compatible: every consumer keeps getting a real
    working tree and a logical-repo slug, and rows still dedupe to one per repo.
  - **`RepoInfo.worktree: Option<String>`** is `None` for the default logical
    row; only a caller that explicitly requests per-worktree rows enumerates via
    `git worktree list --porcelain` and sets `worktree = Some(name)` with `path`
    pointing at that worktree.
  - **Configurable depth.** `RepoDiscovery` is hard-capped at 2 levels today
    (discovery.rs:172-175), but `ls-git-repos` uses infinite-depth `WalkDir`.
    Add a depth control (e.g. `max_depth: Option<usize>`, `None` = unbounded);
    the `org/repo` consumers keep depth 2, `ls-git-repos` requests unbounded, so
    migrating it off `WalkDir` does not silently stop finding deeply-nested repos.
- **Git runner:** every ad-hoc `Command::new("git")` (including clone's private
  `git()`) routes through `common::git::run`/`output`, which captures stderr and
  attaches it to the error context (fixes opaque worktree/clone failures). The
  runner accepts env overrides (e.g. `GIT_SSH_COMMAND` for clone's per-org key).
- **Logging:** `common::log::init(level, project)` takes the level and replaces
  every `env_logger::init()`. It targets **stderr**, matching the current
  behavior (`env_logger`'s default) — switching these short-lived list CLIs to
  file logging is a behavior change and is out of scope. It uses `try_init` so it
  is safe to call from tests without a double-init panic. **No binary defines a
  `--log-level` clap arg today**, so each migrated `main.rs` must add the
  `-l/--log-level` argument to its `Cli` struct and pass the parsed value to
  `init` — the flag is not free, it is wired per crate.

### Implementation Plan

#### Phase 1: Remove the vestigial `filter-ref` crate
**Model:** sonnet
- `git rm -r filter-ref/` (recoverable via history — not `rkvr`, since version
  control is the recovery mechanism for a tracked crate). Remove it from the
  workspace `members` in the root `Cargo.toml` and from the project list in
  `.github/workflows/binary-release.yml:50` (that list is already stale — it
  reads `stale-branches` and omits several tools; correct it to the real binary
  set while here).
- This drops one of the two `git2` users; `common`'s `git2` dep is already dead
  (declared, unused) and `reposlug`'s use is removed in Phase 3 — so `git2` is
  eliminated workspace-wide in Phase 6.
- `otto ci` green.

#### Phase 2: Add the primitives to `common` (no consumer changes yet)
**Model:** opus
- Create `common::git::spec` (move clone's `parse_repospec`/`extract_org_repo_from_path`
  + tests verbatim), `common::git::run`/`output` (both with `envs`),
  `common::log::init`. Keep the old `parse_git_url` as a thin shim temporarily.
- The runner builds `GIT_SSH_COMMAND` with the key path **shell-quoted** (clone's
  current `format!("/usr/bin/ssh -i {}", key)` breaks on a key path containing
  spaces) — fix it once, here, in the centralized path.
- Upgrade `RepoDiscovery`/`RepoInfo` (additively, no consumer behavior change
  yet): `is_git_repo` file-or-dir, bare-container recognition, `RepoInfo.path` =
  canonical default-branch worktree for containers, the opt-in
  `RepoInfo.worktree` field, and a `max_depth` control (default 2, `None` =
  unbounded). The worktree doc consumes these; it does not re-implement them.
- Bump `common` to edition 2024.
- `otto ci` green; `common` tests cover the promoted parser + a mixed flat /
  bare-container discovery fixture.

#### Phase 3: Migrate the "good citizens" + reposlug
**Model:** sonnet
- `reposlug` → `common::git::get_repo_slug_from_path` (the existing shell-out
  helper that reads `origin` from disk and parses it), deleting its
  `git2::Repository::discover` lookup *and* its parser. This removes the second
  and final `git2` user. `ls-stale-branches`, `ls-stale-prs`, `ls-owners` →
  route git calls through `common::git::run`/`output`.
- **Per-crate `--log-level` wiring:** each migrated `main.rs` (`reposlug`,
  `ls-stale-branches`, `ls-stale-prs`, `ls-owners`, `ls-github-repos`) adds an
  `-l/--log-level` clap arg and passes it to `common::log::init` — no binary has
  this flag today, so it is added per crate, not assumed.
- Bump any 2021 crate touched in this phase to edition 2024.

#### Phase 4: Migrate `ls-git-repos` onto `RepoDiscovery`
**Model:** opus
- Replace `WalkDir` + `parse_git_config` with `RepoDiscovery`, requesting
  **`max_depth = None` (unbounded)** — `WalkDir` is infinite-depth today, so
  using `RepoDiscovery`'s default 2-level cap would silently stop finding repos
  nested 3+ deep and break its purpose. Preserve its "repos on disk" output
  (logical repo per container). Delete the GitHub regex; add its `--log-level`
  clap arg.

#### Phase 5: Migrate `clone`
**Model:** opus
- `clone` gains a `common` dependency; its `parse_repospec` is **deleted** and
  imported from `common::git`, its private `git()` helper replaced by
  `common::git::run`/`output`, `env_logger` by `common::log`.
- **Seam with the worktree doc (avoid double-ownership):** this phase only swaps
  clone's *internal calls* to the shared primitives — it does **not** restructure
  clone into `lib.rs` + module split. That module decomposition is the worktree
  doc's Phase 1 and happens on top of an already-consuming `clone`. So this doc
  ships the consumption; the worktree doc ships the shape.

#### Phase 6: Remove duplicates + retire shims + drop `git2`
**Model:** sonnet
- Delete the temporary `parse_git_url` shim once unused; **remove the `git2`
  dependency** from `common`, `reposlug`, and the workspace root `Cargo.toml`
  (no code uses it after Phases 1 and 3). Confirm no crate hand-rolls a parser,
  walker, git invocation, or `env_logger::init`, and that all eight remaining
  crates are on edition 2024. `otto ci`.

## Alternatives Considered

### Alternative 1: Leave as-is (status quo)
- **Pros:** No churn.
- **Cons:** Four parsers drift (the GitHub-only ones already lag clone's),
  discovery logic forks, stderr handling stays inconsistent, `RUST_LOG`
  violation persists, and the worktree work would add a fifth parser / second
  discovery path.
- **Why not chosen:** The divergence is already causing review findings; the
  worktree feature would compound it.

### Alternative 2: New separate "infra" crate distinct from `common`
- **Pros:** Clean name.
- **Cons:** `common` already *is* this crate and is already a dependency of five
  binaries; a second crate splits the seam further.
- **Why not chosen:** Consolidate into the existing seam, don't add another.

## Technical Considerations

### Dependencies
- No new external crates. `ini`, `walkdir`, `regex` usages shrink as duplicates
  are deleted; **`git2` is removed entirely** (Phase 6). `clone` and `reposlug`
  gain a `common` path dep (`filter-ref`, the other non-consumer, is deleted).

### Performance
- Neutral. Discovery gains one `git worktree list --porcelain` per bare container
  (only relevant once worktrees exist); `ParallelExecutor` usage is unchanged.

### Security
- The git runner centralizes env handling (`GIT_SSH_COMMAND`), so the per-org SSH
  key path has one audited code path instead of clone's private one.

### Testing Strategy
- Promoted parser keeps clone's full test suite, moved into `common`. New tests:
  `parse_repospec`/`get_repo_slug_from_path` for non-GitHub hosts, `git::run` stderr capture on a failing
  command, discovery over a mixed flat + bare-container fixture, `log::init` path
  resolution behind an `ENV_LOCK`. Each migrated binary keeps its existing tests
  green (behavior parity is the acceptance bar).

### Rollout Plan
- Phased per above; each phase is independently shippable and `otto ci`-green.
  Behavior parity for end users except: `reposlug`/`ls-git-repos` gain
  host-agnostic parsing (additive), and every tool moves from `RUST_LOG` to
  `--log-level` (documented in each `--help`).
- **Sequencing with the worktree doc.** This consolidation lands **first**:
  Phases 1–5 delete `filter-ref`, give `clone` a `common` dependency and the
  shared parser/runner/log, and add bare-container awareness to `RepoDiscovery`.
  The worktree doc then builds on that base (its `lib.rs` split, bare setup,
  `--worktree`, `--migrate`). Shipping order is: shared-infra Phases 1–6 →
  worktree Phases 1–5.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Promoted parser changes a slug for some input | Low | Med | clone's parser is a superset of the GitHub-only ones; port its test suite verbatim and add non-GitHub cases |
| `RUST_LOG`→`--log-level` breaks a user's muscle memory / scripts | Med | Low | Document in each `--help`; keep `-l`/`--log-level` consistent across all bins |
| Discovery `RepoInfo` change ripples to `ls-stale-*`/`ls-owners` | Med | Med | Default stays one row per logical repo; per-worktree is opt-in; parity tests per consumer |
| Large multi-crate churn introduces regressions | Med | Med | Phase per crate, `otto ci` between phases, behavior-parity tests as the bar |
| Deleting `filter-ref` removes a tool that is secretly in use | Low | Low | No tests/docs/callers; superseded by `ls-stale-branches`; recoverable via git history; `binary-release.yml` reference removed in the same phase |
| `git2` removal misses a live use | Low | Med | Only `filter-ref` (deleted) and `reposlug` (migrated in Phase 3) use it; `common`'s is dead; grep-gate for `git2::` before dropping the dep in Phase 6 |

## Open Questions
- [ ] Should the GitHub-only `parse_git_url` be deleted outright after migration,
      or kept as a documented convenience alias? Proposed: delete if unused.
- [ ] Shared `build.rs` (eight copies today) — worth a tiny `common`-side helper
      or leave duplicated? Proposed: leave for now (low value, noted).

## References
- Workspace audit (2026-06-21): parser/discovery/runner/logging divergence.
- `2026-06-21-clone-bare-worktree.md` — depends on the discovery + parser + runner
  primitives consolidated here.
- `git-tools/CLAUDE.md`; project Rust rules (shell/core split, `--log-level`,
  no `RUST_LOG`, typed errors).
