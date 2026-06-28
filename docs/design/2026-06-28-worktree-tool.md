# Design Document: `worktree` — bare-container navigation tool

**Author:** Scott A. Idler
**Date:** 2026-06-28
**Status:** Implemented (Phases 1-6, 2026-06-28) — `otto ci` green
**Review Passes Completed:** 5/5 (+ cross-model panel: Architect/Gemini + Staff Engineer/Codex, 2026-06-28, findings incorporated)

## Summary

A dedicated `worktree` workspace crate that switches to, creates, lists, and
prunes worktrees inside a bare container (the `.bare/` + `.git`-pointer +
per-branch-worktree layout `clone` produces). It is driven like `clone`: the
binary prints a destination path to stdout and a `worktree()` shell function
`cd`s into it. It replaces the removed `chpwd` navigation magic and supersedes
the misplaced `clone --worktree` flag (removed in Phase 4) with an explicit,
predictable verb.

## Problem Statement

### Background

`clone` produces a bare-container layout: `~/repos/<org>/<repo>/` holding
`.bare/` (the git database), a `.git` pointer file, and one worktree directory
per checked-out branch. This layout is the canonical "bare + worktrees" pattern
(independently arrived at by Cugerone, Nick Nisi, metal3d, gitworktree.org) and
is excellent for parallel work and parallel AI agents, because every branch gets
an isolated working tree that shares one object database.

Two things were bolted onto this that did not work:

1. **A `chpwd` navigation shim** in `shell-functions.sh` that redirected every
   `cd`/`z`/pushd into a bare container's default worktree. Its OLDPWD-based
   "directional carve-out," combined with zoxide wrapping `cd`, silently left
   you standing on the *invisible* bare root, where relative `cd ..` resolved to
   the org dir (two levels above the worktree) and `cd ../sibling` pointed at the
   wrong level. It was removed (2026-06-27).

2. **`clone --worktree <branch>`** — worktree creation hung off the clone tool.
   `clone` is a `git clone` replacement; growing a worktree-management surface on
   it is the wrong home.

Research across ~20 tools and the major blogs/videos converged on one answer:
nobody navigates worktrees with a `chpwd` hook; everybody uses an **explicit
verb** (`wt switch`, `git wt`, an fzf picker, or a `get`-path primitive), almost
always with the "binary prints a path → shell function `cd`s" contract — exactly
what `clone` already uses.

### Problem

There is no predictable, first-class way to navigate and manage the worktrees in
a bare container, and the directory count grows without a cleanup affordance.

### Goals

- Switch to (or create) a worktree for a branch with one command, landing you in
  it — using the proven binary-prints-path / function-`cd`s contract.
- List the container's worktrees.
- Tame the worktree-directory explosion with a recoverable cleanup of merged /
  gone worktrees.
- Keep `clone` a pure `git clone` replacement; move worktree concerns out of it.
- Introduce **no** new invisible magic: no `cd` interception, no `git` shadowing.

### Non-Goals

- GitButler-style virtual branches / single-working-directory model (evaluated
  and declined: same-file conflicts between parallel agents, GUI-first).
- A `git wt` alias or a `git()` wrapper function that shadows `git` (rejected as
  another invisible layer over a command the user already wraps with zoxide).
- Any `chpwd`/auto-`cd` hook (just removed; the whole point is to not bring it
  back).
- Managing flat (legacy single-checkout) repos — `worktree` requires the bare
  layout.
- Cloning, migrating, or version-bumping (those stay in `clone`).

## Proposed Solution

### Overview

A new `worktree` workspace member builds a `worktree` binary. A `worktree()`
shell function wraps it:

- `worktree <branch>` → switch-or-create the worktree, print its path to stdout;
  the function `cd`s into it.
- `worktree` (no arg) → list the container's worktrees (later: fzf picker).
- `worktree --prune` (or a `clean` form) → remove merged/gone worktrees,
  recoverably via `rkvr`.
- Flags and the no-arg form pass straight through the shell function (no `cd`); a
  branch argument is the only form that captures stdout and `cd`s. A branch never
  starts with `-`, so the dispatch is unambiguous.

### Architecture

```
worktree/                      # new workspace member, mirrors clone's layout
  build.rs                     # GIT_DESCRIBE (copied from clone)
  Cargo.toml                   # workspace-inherited deps + version
  src/
    main.rs                    # thin shell: parse, log init, run, print
    lib.rs                     # run() dispatch + Outcome
    cli.rs                     # clap derive
    config.rs                  # TryFrom<Cli> -> Config (Op)
    bare.rs                    # is_bare_container, resolve_container_from_cwd, default_branch
    switch.rs                  # switch-or-create a worktree
    list.rs                    # parse `git worktree list --porcelain`
    clean.rs                   # (future) prune merged/gone worktrees via rkvr
```

All git invocations route through `common::git::{run,output}`; branch slugging
through `common::git::slugify_branch`. The container is resolved from CWD via
`git rev-parse --git-common-dir` (returns `<container>/.bare` from inside any
worktree; the container is its parent).

The `worktree()` shell function lives in `shell-functions.sh` next to `clone()`,
and is wired the same way (manifest `cargo:` install + `link:` of
`shell-functions.sh`).

### Data Model

```rust
// config.rs
pub enum Op { List, Switch(String) }       // future: Prune
pub struct Config {
    pub op: Op,
    pub default_branch: Option<String>,
    // future (Phase 4): per-org SSH key resolved from clone.cfg, so the
    // pre-prune fetch authenticates like clone/migrate.
    // pub ssh_key: Option<PathBuf>,
}

// list.rs
pub struct Entry {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub bare: bool,
    // future (Phase 3): parsed from the porcelain `locked` line, so prune skips
    // locked worktrees and list can mark them.
    // pub locked: bool,
}

// lib.rs
pub enum Outcome { Listed(Vec<Entry>), Switched(PathBuf) }  // future: Pruned(Vec<PathBuf>)
```

### API Design

CLI:

```
worktree [BRANCH] [--default-branch <name>] [-l <level>]
```

- `BRANCH` present → `Op::Switch(branch)`; absent → `Op::List`.
- Branch-source selection in `switch`:
  1. existing local head → check out as-is into a slugified dir;
  2. existing remote-only branch → create a tracking local branch (real name),
     slugified dir, `--track` (handles the bare-clone upstream gotcha). Note: a
     `git clone --bare` copies the remote's heads into local `refs/heads/*`, so
     this path only fires for branches that appeared on origin *after* the clone
     (discovered by a later fetch); branches present at clone time take path 1;
  3. new branch → slugify, use the slug as both branch and dir, based on the
     default branch.
- Idempotent: re-running for a branch whose worktree exists returns it — but
  only after verifying that dir's checked-out branch matches the intended branch
  (Phase 2 guard against the slug-collision where `feature/auth` and a literal
  `feature-auth` share a slug); a mismatch is a hard error, never a silent `cd`.

Shell contract: stdout is **only** the destination path for the switch form;
list prints a human table to stdout (passed straight through, never `cd`-ed).

### Implementation Plan

> Reordered after the 2026-06-28 cross-model review (Architect + Staff Engineer):
> correctness + shared primitives now precede the features that depend on them,
> and the Phase 3/4 contracts that both reviewers flagged as broken-as-written
> are corrected here.

#### Phase 1: Crate scaffold + switch + list + shell function (DONE)
**Model:** sonnet
- New `worktree` workspace member mirroring `clone`'s layout (not `scaffold`d).
- `switch` (lifted from the proven `clone::worktree::add`) + `list`.
- `worktree()` shell function; CLAUDE.md + memory updated.
- 10 crate tests + 7 shell-dispatch tests; `otto ci` green.
- **Known inherited bug (fixed in Phase 2):** the slug-collision in
  `ensure_or_add` below.

#### Phase 2: Correctness hardening + `common::bare`
**Model:** opus
- **Fix the slug-collision idempotency bug** (review CRITICAL/MAJOR, present in
  shipped Phase 1 and in `clone::worktree`). `ensure_or_add` reuses an existing
  `<container>/<slug>/` whenever its `.git` is a file, without checking *which*
  branch is checked out there. Because `slugify_branch` collapses `/`, spaces,
  dots, and hyphens into one namespace, `feature/auth` and a literal
  `feature-auth` map to the same dir — so `worktree feature-auth` can silently
  `cd` into the `feature/auth` tree. Fix: before reusing a dir, verify its
  checked-out branch equals the intended branch (`git -C <dir> symbolic-ref
  --short HEAD`); on mismatch, bail with a clear "dir X already hosts branch Y"
  error. Apply to both `worktree::switch` and `clone::worktree::add`.
- **Promote shared bare primitives to `common::bare`** (resolves the Open
  Question as *yes*). Move `is_bare_container`, `default_branch`, and
  `symbolic_ref_short` from `clone::bare` (and the duplicate in `worktree::bare`)
  into `common::bare`; both crates consume it. Rationale: `default_branch` makes
  a *mutating* call (`git remote set-head origin -a`) — duplicating mutating
  state logic across binaries guarantees drift. Phase 4 (`--prune`) depends on
  this, so it lands first.

#### Phase 3: fzf picker for the no-arg / interactive form
**Model:** sonnet
- **No-arg is ALWAYS the interactive switcher** (review CRITICAL — the v1
  "polymorphic stdout" idea is unworkable: stdout is captured into a pipe by
  `dest=$(eval $WORKTREE …)`, so `stdout.is_terminal()` is *always false* under
  `$()` and a table would be swallowed by the `[[ -d "$dest" ]]` guard).
  - `worktree` (no arg) → fzf picker over the container's worktrees. fzf draws
    its UI on `/dev/tty` (or stderr) and the binary captures fzf's stdout
    internally, emitting **only** the selected path on its own stdout → `cd`.
  - Detect interactivity by testing **stdin / `/dev/tty`**, NOT stdout.
  - No fzf, or no tty → **fail explicitly** pointing at `worktree --list`; do
    NOT silently degrade (silent degrade is what made it polymorphic).
  - `worktree --list` (or `ls`) → the human table (passthrough, no `cd`).
- Shell dispatch update: no-arg moves to the capture-and-`cd` case; `--list`/`ls`
  stays passthrough. Update `tests/shell-functions.zsh` accordingly.
- Add `locked: bool` to `list::Entry` by parsing the porcelain `locked` line (so
  Phase 4 can skip locked worktrees, and `list` can mark them).

#### Phase 4: `worktree --prune` — cleanup of merged/gone worktrees
**Model:** opus
- **State the prune invariant first** (review CRITICAL — both reviewers): the
  branch *ref always survives*; prune removes only the checkout directory. A
  clean, merged worktree's commits live in the bare DB, so removal is trivially
  recoverable by re-adding the worktree — `git worktree remove` is sufficient and
  `rkvr` is not needed for the clean case. (The `rkvr rmrf` + `git worktree
  prune` combo proposed earlier does NOT yield recoverability: `git worktree
  prune` permanently removes the `.bare/worktrees/<name>/` admin dir, so an
  rkvr-restored work dir points at a dead admin dir → fatal "not a git
  repository". `clone::migrate`'s rkvr use is not transferable — its dirty/
  unmerged/rescue/verify/rollback logic is migration-specific and prune inherits
  none of it.)
- **Detection base is `origin/<default>`, not local** (review CRITICAL):
  `git branch --merged origin/<default>` after a `git fetch --prune`, because a
  fetch advances `origin/<default>` while local `<default>` lags (and work repos
  may never check out / push `main` locally). "Gone" upstream via
  `git for-each-ref --format '%(upstream:track)'` showing `[gone]` (only
  meaningful for branches with upstream config, post-fetch-prune).
- **Hard protections, enforced by the tool** (prune does NOT inherit migrate's):
  never remove the default-branch worktree, the current worktree, any worktree
  with uncommitted changes (`git status --porcelain`), any worktree with unpushed
  commits, or any locked worktree.
- **Network/auth** (review MAJOR): the pre-prune fetch must use the same per-org
  SSH key resolution as `clone`/`migrate` (load `clone.cfg`, resolve the key,
  set `GIT_SSH_COMMAND`) — `worktree`'s `Config` gains the `ssh_key` field for
  this. Without it, prune on a private repo diverges from `clone`.
- **Observability** (per `cli.md`: no `--dry-run` on an opt-in destructive flag):
  print a removal summary and require confirmation before removing; recovery is
  via git re-add (clean invariant), not a dry-run preview.

#### Phase 5: Rehome `clone --worktree` into `worktree`
**Model:** opus
- Remove `--worktree` from `clone` (CLI, `Op::AddWorktree`, `clone::worktree`,
  its tests, the validation rules in `config.rs`). `common::bare` already exists
  (Phase 2), so this is deletion + rewiring, not extraction.
- Update CLAUDE.md / AGENTS.md / clone's `--help` to point at `worktree`.

#### Phase 6: Install wiring (required) + docs
**Model:** sonnet
- **Add `worktree` to the `manifest.yml` `scottidler/git-tools` `cargo:` list**
  (review MAJOR — reproducible install is dotfiles-manifest-owned, NOT repo
  `otto install`; the manifest currently omits `worktree`). The shell function
  ships via the existing `shell-functions.sh` `link:`.
- README/CLAUDE.md examples; field-guide via `cli-shakedown`.

## Alternatives Considered

### Alternative 1: `git-wt` binary + `git()` wrapper (k1LoW model)
- **Description:** name the binary `git-wt` so `git wt` dispatches to it; install
  a `git()` shell wrapper that intercepts `git wt` and `cd`s.
- **Pros:** the `git wt <branch>` spelling; one wrapper.
- **Cons:** shadows *every* `git` invocation with a shell function — a third
  layer of indirection on top of zoxide's `cd` wrapper and the `clone()`
  function. `git wt` (a subprocess) can never `cd` you anyway, so the wrapper is
  mandatory, not optional.
- **Why not chosen:** Scott explicitly rejected adding more invisible magic over
  `git`. A standalone `worktree` verb shadows nothing.

### Alternative 2: keep the `chpwd` navigation hook (fix it instead of removing)
- **Description:** repair the OLDPWD carve-out so `cd` into a container is
  predictable.
- **Pros:** "just `cd`" ergonomics.
- **Cons:** interacts badly with zoxide-as-`cd` (silent frecency teleport on a
  miss) and leaves the user on an invisible bare root; the failure mode that
  triggered this whole effort.
- **Why not chosen:** removed for cause; the ecosystem unanimously uses explicit
  verbs, not `cd` interception.

### Alternative 3: GitButler (virtual / parallel branches)
- **Description:** one working directory, multiple branches overlaid; no worktree
  dirs at all.
- **Pros:** deletes the directory-explosion and navigation problem outright.
- **Cons:** same-file edits across lanes "get messy" (races) — exactly the
  parallel-agent case; GUI-first; off the terminal-native Rust toolchain.
- **Why not chosen:** declined for now (see the navigation-direction memo).

### Alternative 4: pure-shell fzf function (no binary)
- **Description:** a zsh function running `git worktree list | fzf | cd`.
- **Pros:** zero Rust.
- **Cons:** the bare-aware logic (container resolution, slugify, branch-source
  selection, `--track`, merged/gone detection, rkvr-safe pruning) does not belong
  in shell; it is already tested Rust.
- **Why not chosen:** keep the logic in the binary; the shell function stays a
  thin `cd` wrapper.

## Technical Considerations

### Dependencies
- Internal: `common` (git plumbing, slugify). No new workspace deps beyond what
  `clone` uses (`clap`, `eyre`, `log`).
- External runtime: `git` (always); `fzf` (Phase 2, optional — degrade to plain
  list when absent); `rkvr` (Phase 3, required for `--prune`, enforced by a
  preflight like `clone::migrate`).

### Performance
- All work is a handful of `git` subprocess calls per invocation; negligible.

### Security
- **Persona invariant:** worktrees stay under `~/repos/<org>/`, so the
  `~/.gitconfig` `includeIf "gitdir:~/repos/tatari-tv/"` still fires. `worktree`
  only ever creates dirs inside the resolved container, preserving this.
- **Recoverability:** pruning routes through `rkvr rmrf`, never a raw delete,
  matching the repo safety rule. Refuse to prune without `rkvr`.

### Testing Strategy
- `tempfile` bare-container fixtures built with `git clone --bare` from a local
  source (no network), mirroring `clone`'s tests. Cover: new/local/remote-only
  branch switching, slugging, idempotency, list parsing (porcelain + real
  container), and (Phase 3) prune protections (dirty/default/current never
  removed).
- Shell-dispatch tests in `tests/shell-functions.zsh` (stubbed binary) verify the
  `worktree()` routing (branch → `cd`; no-arg/flags → passthrough).

### Rollout Plan
- Ships with the workspace via `otto install`; the shell function via the
  existing `shell-functions.sh` manifest link. No separate installer.
- `clone --worktree` stays functional until Phase 4 removes it, so there is no
  flag-day for muscle memory.

### Edge Cases & Limitations
- **Branch names starting with `-`** are not reachable through the shell function
  (the `-*` dispatch case routes to passthrough, no `cd`). Pathological; accepted
  limitation. The binary itself still handles them if invoked directly.
- **Commitless / empty container** (freshly created remote, no branches): `list`
  shows only the bare entry; `switch` bases a new branch on the resolved default
  once one exists. No crash.
- **Stale/prunable worktree entries** (someone hand-deleted a worktree dir):
  `git worktree list --porcelain` emits a `prunable` line; `list` must tolerate
  unknown lines (the current parser ignores any line it doesn't recognize, so
  this is already safe) and `--prune` should `git worktree prune` these admin
  entries too.
- **Not a bare container / outside any repo:** `resolve_container_from_cwd` bails
  ("not inside a git repository"); a flat checkout bails ("not a bare
  container"). Both are clear, non-mutating errors.
- **Same branch as local head and remote-tracking:** path 1 (local) wins, which
  is correct — the local head is authoritative.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `--prune` removes a worktree the user still wanted | Low | High | Branch-ref-survives invariant + hard protections (default/current/dirty/unpushed/locked) + confirmation summary; clean+merged is git-recoverable by re-add |
| Prune base (`local default`) misses GitHub-side merges | High | High | Compare against `origin/<default>` after `git fetch --prune` (Phase 4) |
| Slug collision silently `cd`s into the wrong worktree | Med | Med | Phase 2 verifies the dir's checked-out branch before reuse; bail on mismatch |
| Phase 3 no-arg "polymorphic" output breaks the wrapper | High | High | No-arg is always the interactive switcher; detect tty via stdin/`/dev/tty`, fail explicitly without fzf (Phase 3) |
| Prune fetch on a private repo fails / wrong identity | Med | Med | Resolve per-org SSH key from `clone.cfg` like clone/migrate (Phase 4) |
| `fzf` absent | Med | Low | Explicit error pointing at `--list`; never silently degrade |
| Removing `clone --worktree` breaks muscle memory | Med | Low | Keep it until Phase 5; clone prints a pointer to `worktree` |
| Duplicated bare helpers drift between `clone` and `worktree` | Med | Med | Phase 2 promotes them to `common::bare` (esp. the mutating `default_branch`) |

## Open Questions

Resolved by the 2026-06-28 review (kept for the record):
- [x] No-arg = fzf picker (always, when interactive), `--list`/`ls` = table —
      *not* polymorphic. (CRITICAL finding.)
- [x] Promote shared bare primitives to `common::bare` — yes, in Phase 2
      (the mutating `default_branch` makes duplication a drift hazard).
- [x] Locked worktrees: parse the porcelain `locked` line into `Entry`
      (Phase 3), skip them in prune.

Still open:
- [ ] Command surface for prune: a `--prune` flag vs a `clean`/`rm` subcommand.
      Flags keep the shell dispatch trivial (`-*` passthrough); subcommands read
      better but complicate routing. (`cli.md` forbids a `--dry-run` companion
      either way.)
- [ ] Prune confirmation UX: interactive y/N prompt vs a `--yes` to skip, and
      whether the summary goes to stderr (so it never pollutes the stdout
      contract).
- [ ] Phase 5 sequencing: rehome `clone --worktree` before or after `--prune`
      ships — i.e. how long the two creation paths coexist.

## References
- `docs/design/2026-06-21-clone-bare-worktree.md` — the bare-container layout.
- Navigation-direction memo (project memory) — why chpwd was removed and
  GitButler declined.
- Research (this session): Cugerone, Nick Nisi, metal3d, gitworktree.org
  (canonical `.bare` layout); k1LoW/git-wt, gwq, gwm, workmux, yankeexe/wt
  (navigation + cleanup patterns); Trigger.dev (GitButler tradeoff).
- Cross-model review (2026-06-28): raw outputs at
  `/tmp/review-panel/f9rExka0/arch.out` (Architect/Gemini) and
  `/tmp/review-panel/f9rExka0/staff.out` (Staff Engineer/Codex).
