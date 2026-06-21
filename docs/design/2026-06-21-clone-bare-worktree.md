# Design Document: Worktree-Savvy `clone` (Bare-Repo + Nested Worktrees)

**Author:** Scott A. Idler
**Date:** 2026-06-21
**Status:** Implemented
**Review Passes Completed:** 5/5
**Depends on:** `2026-06-21-common-shared-infra.md` (the shared parser, git
runner, and `RepoDiscovery` this design extends with bare-container awareness)

## Summary

Make the `clone` tool set up repositories in the **bare-repo + nested-worktrees**
layout instead of a flat single checkout. `clone org/repo` will create a
"no-files" container (`.bare/` holding only the git database, a `.git` pointer
file, and a checked-out worktree on the default branch), `clone --worktree
<branch>` will add further worktrees, the shared `common::RepoDiscovery` will
gain bare-container awareness, and a `clone --migrate` path will convert existing
flat checkouts. The persona model (path-driven commit identity + per-org SSH
keys) is preserved by construction.

**Repo-path contract (resolved):** `~/repos/<org>/<repo>` is the **logical repo**
that tools reason about (one row in discovery), and `~/repos/<org>/<repo>/<default-branch>`
is the **canonical working tree** you `cd` into. The default-branch worktree
(typically `main`) is a **guaranteed invariant** — always present in a bare
container — so "the repo" always resolves to a real working tree. Per-worktree
enumeration is an opt-in, never the default.

## Problem Statement

### Background

`clone` is the single entry point for every repository on this machine. It
clones into `~/repos/<org>/<repo>`, drives a per-org SSH transport key from
`~/.config/clone/clone.cfg`, prints the destination path to stdout, and a zsh
wrapper `cd`s into it. Commit identity is **not** set per-repo: it resolves from
`~/.gitconfig` with an `includeIf "gitdir:~/repos/tatari-tv/"` that swaps in the
work identity for anything under the `tatari-tv` org. Today every repo is a
flat, single-branch checkout.

Agentic AI workflows (loopr, td, Claude Code) and ordinary context-switching
both want **multiple branches checked out at once on the same machine** without
stashing or re-cloning. Git worktrees solve this, and the cleanest community
pattern is the *bare-repo + nested-worktrees* layout: a single git database
shared by N peer worktrees, with no privileged "main" checkout to conflict over.

### Problem

There is no first-class way to lay a repo down so that adding/removing worktrees
is trivial and persona-safe. Doing it by hand is error-prone: a `git clone
--bare` silently leaves the fetch refspec empty (remote-tracking branches never
populate), and putting worktrees in the "wrong" place silently breaks the
`includeIf gitdir:` persona resolution.

### Goals

- `clone org/repo` produces a bare container with a ready default-branch worktree
  and `cd`s the user into that worktree.
- `clone --worktree <branch> [org/repo]` adds a worktree to an existing
  container and `cd`s into it.
- The default-branch worktree is always present (invariant); navigating to a
  container by any means lands you in it.
- Every worktree keeps the correct persona identity automatically.
- `common::RepoDiscovery` recognizes bare containers and reports each as **one
  logical repo** (matching `ls-git-repos`'s "what repos do I have on disk?"
  purpose), alongside legacy flat clones (mixed ecosystem). The discovery
  primitive itself is consolidated in the shared-infra doc; this design adds the
  bare-container awareness.
- A `clone --migrate` path converts an existing flat checkout into a bare
  container without losing local work.
- Extract a testable `lib.rs` from the current 714-line `main.rs` as part of the
  work (shell/core split), consuming the shared `common::git` parser + runner.

### Non-Goals

- Auto-migrating the existing fleet of flat clones. Migration is explicit,
  per-repo, opt-in.
- A general worktree lifecycle manager (rename, prune, lock, move). Only
  add-on-clone and add-via-flag are in scope; teardown is `git worktree remove`.
- Changing the persona mechanism itself (gitconfig `includeIf`, clone.cfg SSH
  keys). The design preserves it; it does not redesign it.
- Supporting worktree layouts other than bare-nested (no sibling-dir or
  inside-repo variants).

## Proposed Solution

### Overview

Introduce a **layout** concept with two values: `bare` (new default) and `flat`
(legacy single checkout, opt-out escape hatch). The default flips to `bare`.
`clone` grows two new operations (`--worktree`, `--migrate`) and the binary is
restructured into a thin `main.rs` over a testable `lib.rs` + focused modules.

### Architecture

On-disk layout produced by `clone org/repo` (bare):

```
~/repos/<org>/<repo>/        # the container
  .bare/                     # git clone --bare → database only, no working files
  .git                       # a FILE, one line: "gitdir: ./.bare"
  <default-branch>/          # git worktree add → the worktree cd lands in
```

After `clone --worktree feature-x org/repo`:

```
~/repos/<org>/<repo>/
  .bare/
  .git
  main/
  feature-x/                 # new worktree, cd lands here
```

Persona invariant (verified): from inside any worktree, `git rev-parse
--git-dir` resolves to `<container>/.bare/worktrees/<name>` and `--git-common-dir`
to `<container>/.bare`. Both stay under `~/repos/<org>/`, so `includeIf
gitdir:~/repos/tatari-tv/` fires and commits get the work identity. **Worktrees
must never be placed outside `~/repos/<org>/` or identity silently reverts to
home.** This is an enforced invariant, not a convention.

### Repo-path contract, invariants, and navigation

- **`main` is always present.** The default-branch worktree is a guaranteed
  invariant of a bare container: `clone` creates it, and operations never leave a
  container without it. So "the repo" always resolves to a real working tree, and
  the "what if there's no canonical worktree" edge cannot occur.
- **Container = logical repo.** `~/repos/<org>/<repo>` is the identity tools
  reason about; discovery emits it once. `<repo>/<default-branch>` is the working
  tree `cd` lands in.
- **Navigation shim (in scope).** `clone`'s wrapper already `cd`s into the
  default worktree on a fresh clone, but every *other* way of landing in a repo
  (`cd ~/repos/org/repo`, `z repo`, an IDE bookmark) would drop you in the bare
  container with no working files. To remove that friction, the shell layer gains
  a `cd`/`z`-style shim (in `shell-functions.sh`, peer to the existing `clone`
  wrapper): when the target is a bare container, it redirects into the
  default-branch worktree. This is what makes bare-as-default ergonomically
  transparent — you keep landing "in the repo," now meaning its `main/` worktree.

### The bare-clone setup sequence (proven in prototype)

1. `git clone --bare <remote>/<org>/<repo> <container>/.bare` (per-org SSH key
   via `GIT_SSH_COMMAND`, SSH→HTTPS fallback, exactly as today).
2. Write `<container>/.git` containing `gitdir: ./.bare`.
3. **Mandatory refspec fix.** A `--bare` clone leaves `remote.origin.fetch`
   empty, so `git branch -r` is empty and worktrees cannot track `origin/*`:
   ```
   git config remote.origin.fetch '+refs/heads/*:refs/remotes/origin/*'
   git fetch origin
   ```
4. Detect the default branch from `git symbolic-ref refs/remotes/origin/HEAD`
   (strip `refs/remotes/origin/`). If unset, run `git remote set-head origin -a`
   first; fall back to clone.cfg `[clone] default` only as a last resort. Never
   hardcode `main`.
5. `git worktree add <default-branch-dir> <default-branch>`.
6. Print the worktree path to stdout (the wrapper `cd`s into it).

### Worktree-add sequence (`--worktree <branch>`)

Resolve the container: if `org/repo` is given, it is `clonepath/org/repo`;
otherwise derive it from CWD via `git rev-parse --git-common-dir` (which returns
`<container>/.bare` from anywhere inside any worktree) and take its parent. Then
choose the branch source explicitly:

Lookup order uses the **raw** argument first, so existing non-slug branches stay
reachable (a reviewer caught that slugifying first would lock you out of a
colleague's `feature/auth-fix`):

1. raw arg matches `refs/heads/<arg>` → `git worktree add <dir> <arg>`.
2. else raw arg matches `refs/remotes/origin/<arg>` → `git worktree add -b <arg>
   --track <dir> origin/<arg>`.
3. else (**new** branch) → slugify the arg to lowercase-hyphenated (per
   `general.md`: no slashes, no `feature/` prefixes — `Feature/Foo Bar` →
   `feature-foo-bar`) and use that single slug as **both** branch and directory:
   `git worktree add -b <slug> <slug> <default-branch>`.

Naming rule: slugification applies **only when minting a new branch** (steps 3),
so `clone --worktree "Add Auth"` and `clone --worktree add-auth` mint the same
`add-auth`. For an **existing** branch we never rename someone else's branch —
keep its real name for checkout and slugify *only* the directory (so an existing
`release/1.2` checks out as-is into a `release-1-2/` dir). The directory name in
all cases is the checked-out branch name with `/` → `-` for filesystem legality.
Print the worktree path.

### Branch ↔ worktree relationship and the integration flow

Directory name and branch are **independent** git arguments; the tool keeps them
1:1 by slugifying new branches (so the slug is legal as both a branch and a
directory), but that is a convention this tool adopts, not a git constraint. The
one inextricable git rule is: **a branch may be checked out in
only one worktree at a time** (and a branch can only be force-updated from the
worktree that holds it). Both are protections, not obstacles. Verified behavior
(prototype, 2026-06-21) for the rebase/merge-against-main cases this raises:

- **Rebase a feature onto main** — from the feature worktree, `git rebase main`
  succeeds *even though `main` is checked out in the `main/` worktree* (rebase
  reads main's tip, it does not modify or check out main). Prefer `git rebase
  origin/main` so the rebase target does not depend on the local `main` worktree
  being current.
- **Merge main into a feature** — `git merge main` from the feature worktree
  works for the same reason.
- **Integrate (merge a feature into main)** — done *in the `main/` worktree*:
  `cd ../main && git merge feature`. The `main/` worktree is always present,
  checked out, and clean, so integration needs no stash and no branch switch.
- **Forbidden, by design:** checking out `main` in a second worktree, or
  force-updating `main` from a non-`main` worktree, both fail with a clear
  `fatal:` — they cannot silently corrupt main.

Persona interaction: `~/.gitconfig-work` sets `branch.main.pushRemote = no_push`
for `tatari-tv`, so on work repos integration is **push-the-feature + open a
PR**, never a local merge-into-main + push. The `main/` worktree there is a
read/rebase-onto target; on home repos a local merge-into-main is fine. The bare
layout serves both unchanged.

### Discovery (consumed from shared `common`, not implemented here)

**All `RepoDiscovery`/`RepoInfo` changes are owned by
`2026-06-21-common-shared-infra.md`** (sole owner, to avoid double-ownership):
bare-container recognition, `is_git_repo` file-or-dir, the `max_depth` control,
the opt-in `RepoInfo.worktree` field, and — the load-bearing decision — that for
a bare container **`RepoInfo.path` is the canonical default-branch worktree**
(`<container>/<default-branch>`, always present) with `slug = org/repo`. That
keeps `ls-owners` (treats `path` as a working-tree root) and `ls-stale-branches`
(treats `path` as a git cwd) working unchanged while deduping to one logical row.

This design only **relies on** that contract: the bare layout `clone` produces
must be discoverable as one logical repo whose `path` is the default worktree.
The bare-container shape (`.git` file → `.bare`, worktrees as siblings) is what
the shared discovery recognizes; nothing about discovery is implemented in this
doc.

### Migrate sequence (`--migrate`)

Convert a flat `~/repos/<org>/<repo>` into a bare container **without mutating
the original until the result is verified**, preserving local-only branches:

1. Refuse with a clear error if the working tree is dirty (uncommitted or
   untracked) or `git stash list` is non-empty. Never auto-resolve; never lose
   local work. The error tells the user how to proceed: commit/branch the stash
   (`git stash branch <tmp>`) and commit or `.gitignore` the dirty files, then
   re-run `--migrate`.
2. Capture the **real** `origin` URL and the currently checked-out branch.
3. **Clone the bare container from the LOCAL repo, not from `origin`**
   (`git clone --bare <old-repo> <repo>.migrating/.bare`). This is load-bearing
   and was proven (2026-06-21): a clean working tree can be *ahead* of `origin`
   with unpushed commits; cloning the bare from `origin` and skipping
   origin-existing branches would silently drop them, and step 6's delete makes
   it permanent. Cloning from the local repo captures every local ref at its
   local state — all branches (including local-only) and all unpushed commits.
4. Repoint the container at the real remote and populate tracking:
   `git -C <repo>.migrating/.bare remote set-url origin <real-url>`, then the
   refspec fix (`+refs/heads/*:refs/remotes/origin/*`) and `git fetch origin`.
   `fetch` updates `refs/remotes/origin/*` only, so the local-ahead `refs/heads/*`
   from step 3 are preserved. (Verified: local-ahead commits and a local-only
   branch both survive; `origin/*` tracking is correctly populated.)
5. Add a worktree for the previously checked-out branch (the always-present
   default-branch worktree is created regardless); write the container's `.git`
   pointer.
6. Verify the new container resolves (`git -C <new-worktree> status` clean, slug
   matches origin). Then perform a **recoverable, non-destructive swap**:
   `mv <repo> <repo>.backup` → `mv <repo>.migrating <repo>` → re-verify →
   `rkvr rmrf <repo>.backup`. Renaming the old tree aside first (rather than
   `rkvr` then `mv`) means a failure at any step leaves both the original and the
   candidate intact.
7. **Print the canonical default-branch worktree path to stdout** (the wrapper
   `cd`s into it), keeping the stdout-is-only-the-destination contract — `--migrate`
   leaves you standing in the migrated repo's default worktree, exactly like a
   fresh `clone`.

What migrate preserves vs. drops (stated, not left silent): the commit object DB,
all local branches + unpushed commits, and tags travel via the local bare clone.
Submodule gitlinks and LFS pointer objects travel with the object DB, but their
*working* state (submodule checkouts, LFS-smudged files) is re-materialized per
worktree on first checkout, same as any fresh clone. **Not** carried: local
`.git/hooks`, `.git/config` extras beyond `origin` (custom remotes, alternates),
and reflogs — these are machine-local and intentionally not migrated; if a repo
relies on them, `--migrate` warns and lists what it is dropping rather than
copying them silently.

### Data Model

```rust
// cli.rs — parsing only
struct Cli {
    repospec: Option<String>,   // optional: --worktree can run inside a container
    revision: String,           // default "HEAD"
    remote: String,
    clonepath: String,
    mirrorpath: Option<String>,
    versioning: bool,
    flat: bool,                 // opt out of bare layout (legacy single checkout)
    worktree: Option<String>,   // add a worktree for <branch>
    migrate: bool,              // convert a flat checkout to bare
    log_level: LevelFilter,     // replaces RUST_LOG (custom --log-level)
    verbose: bool,
}

// config.rs — validated, via TryFrom<Cli>
enum Layout { Bare, Flat }

struct Config {
    spec: Option<RepoSpec>,
    layout: Layout,             // CLI --flat/--bare > clone.cfg default-layout > Bare
    op: Op,                     // Clone | AddWorktree(String) | Migrate
    remote: String,
    clonepath: PathBuf,
    mirrorpath: Option<PathBuf>,
    revision: String,
    ssh_key: Option<PathBuf>,   // resolved from clone.cfg per org
}
```

(`common::RepoInfo` gains a `worktree: Option<String>` field and the
`path = default-branch worktree` contract for bare containers — both **defined in
the shared-infra doc**, consumed here. `RepoSpec` is likewise imported from
`common::git`.)

### API Design

```rust
// clone/src/lib.rs
pub fn run(config: Config) -> Result<PathBuf>;   // returns the cd destination

// parse_repospec + slugify_branch are imported from common::git (owned by the
// shared-infra doc), not defined in clone.

// clone/src/bare.rs
pub fn setup_bare_container(cfg: &Config, spec: &RepoSpec) -> Result<PathBuf>;     // returns default worktree path
pub fn add_worktree(container: &Path, branch: &str) -> Result<PathBuf>;
pub fn default_branch(container: &Path) -> Result<String>;
pub fn fix_fetch_refspec(container: &Path) -> Result<()>;

// clone/src/migrate.rs
pub fn migrate_flat_to_bare(flat: &Path) -> Result<PathBuf>;   // prints the canonical worktree path

// Discovery (is_bare_container, worktree enumeration, RepoInfo.path/worktree)
// lives in common — see the shared-infra doc; not part of clone's API.
```

CLI surface:

```
clone org/repo                  # bare container + default worktree, cd into it
clone --flat org/repo           # legacy single checkout (old behavior)
clone --worktree feat org/repo  # add worktree 'feat' to the container, cd in
clone --worktree feat           # same, when CWD is already inside a container
clone --migrate org/repo        # convert existing flat checkout to bare
clone --versioning org/repo     # implies --flat (versioned checkout into repo/<sha>)
```

### Implementation Plan

#### Phase 1: Extract `lib.rs` + module split (no behavior change)
**Model:** sonnet
- **Precondition:** the shared-infra doc has already landed, so `clone` already
  depends on `common`, imports `parse_repospec`/`slugify_branch` from
  `common::git`, routes git calls through `common::git::run`/`output`, and logs
  via `common::log` (`--log-level`). This phase does **not** redo that migration.
- Split `main.rs` into `main.rs` (thin shell), `lib.rs`, `cli.rs`, `config.rs`,
  `bare.rs`, `migrate.rs`. Move inline tests to per-module `tests.rs`.
- Verify: `otto ci` green, existing integration tests pass unchanged (still flat).

#### Phase 2: Bare-container setup + default worktree
**Model:** opus
- `bare.rs`: `setup_bare_container`, `fix_fetch_refspec`, `default_branch`,
  `add_worktree`. Wire `Layout::Bare` as the default in `config.rs` with
  `--flat` / clone.cfg `default-layout` override.
- Reuse the existing SSH-key + SSH→HTTPS fallback for the `--bare` clone.
- `clone org/repo` prints the default worktree path; wrapper cds in.

#### Phase 3: `--worktree` flag
**Model:** opus
- Container resolution (CWD-inside or `org/repo` arg), branch-source selection,
  branch/dir slugification, `git worktree add`. Print worktree path.

> Discovery's bare-container awareness is **not** a phase here — it is owned and
> implemented by the shared-infra doc (its Phase 2/4), which lands before this
> work. This doc only ensures `clone` produces the layout that discovery
> recognizes.

#### Phase 4: `--migrate`
**Model:** opus
- `migrate.rs`: dirty/stash refusal gates, **bare-clone-from-local** (preserves
  unpushed commits), repoint-origin + refspec + fetch, non-destructive
  rename-aside swap. Tests: clean migrate, dirty/stash refusal, clean-but-ahead
  branch survives, local-only branch survives.

#### Phase 5: `cd`/`z` navigation shim + docs
**Model:** sonnet
- Add the bare-container redirect shim to `shell-functions.sh` (peer to the
  `clone` wrapper). Update `git-tools/CLAUDE.md` (bare layout, refspec gotcha,
  persona invariant, the repo-path contract), `clone.cfg` template comment, and
  confirm the zsh wrapper still only consumes stdout. `/shipit`.

## Alternatives Considered

### Alternative 1: Sibling worktrees (flat clone stays primary)
- **Description:** Keep `~/repos/<org>/<repo>` as a normal checkout; add
  worktrees as `~/repos/<org>/.<repo>-worktrees/<branch>/`.
- **Pros:** Zero change to the default path; lowest blast radius; discovery
  mostly unaffected.
- **Cons:** Asymmetric (a privileged main checkout); not the "no-files" model the
  user wants; two sibling dirs per repo clutter the org directory.
- **Why not chosen:** User explicitly wants the bare "no-files clone with nested
  worktrees" model.

### Alternative 2: Worktrees nested *inside* the working repo (`repo/.worktrees/`)
- **Description:** Normal clone with worktrees under a gitignored `.worktrees/`.
- **Pros:** Single self-contained dir; identity safe.
- **Cons:** Collides with clone's existing `git clean -xfd` (clone/src/main.rs:343),
  which would delete the worktrees; nested working trees are messy.
- **Why not chosen:** Directly unsafe given current clone behavior.

### Alternative 3: Separate `wt` binary instead of a `clone` flag
- **Description:** New workspace crate for worktree lifecycle; `clone` stays
  single-purpose.
- **Pros:** Cleaner separation of concerns.
- **Cons:** More wiring (manifest, shell function, install); the user asked to
  "make clone savvy," i.e. one tool.
- **Why not chosen:** Explicit user preference for `clone --worktree`.

## Technical Considerations

### Dependencies
- No new crates. Continues to shell out to system `git` (already the pattern).
  `ini` for clone.cfg stays. `rkvr` invoked for migrate deletes (shell out, per
  the safety rule; fall back to std removal + WARN if absent).

### Performance
- Bare clone is a single `git clone --bare` plus one `fetch` (the refspec fix
  re-fetches remote heads — cheap, same objects). Worktree add is local and
  fast. Discovery gains one `git worktree list --porcelain` per bare container.

### Security
- No new network or credential surface. SSH key resolution is unchanged. The
  persona invariant is the security-relevant property: commits in `tatari-tv`
  worktrees must carry the work identity — guaranteed by keeping worktrees under
  the org prefix.

### Testing Strategy
- Unit: `parse_repospec` (unchanged), `slugify_branch` (slashes, spaces, case),
  branch-source selection, default-branch parsing from `symbolic-ref`, layout
  precedence.
- Fixture-based: local "remote" repos (as in the prototype) for bare setup,
  worktree add, discovery enumeration, and migrate (clean / dirty-refusal /
  local-only-branch survival). Env-touching tests serialized behind `ENV_LOCK`.
- Integration: extend `clone/tests/integration_tests.rs` for the bare default
  and `--flat` legacy path. `--migrate` tested against a local fixture, never
  the network.
- **Persona-invariant test (cheap insurance, in scope).** A test that generates a
  bare container with a worktree under a temp dir mimicking `~/repos/tatari-tv/`,
  sets an `includeIf "gitdir:"` against it, and asserts `git -C <worktree> config
  user.email` resolves to the work identity — locking the security-relevant
  property so a future refactor can't silently break persona resolution.

### Edge cases handled
- **Idempotent re-run.** `clone org/repo` on an existing container re-applies the
  refspec fix, fetches, and ensures the default worktree exists, then cds in
  (mirrors today's "update if exists" behavior). `clone --worktree <b>` when
  `<b>`'s worktree already exists just cds into it rather than erroring.
- **Rerun on an existing *flat* clone after the default flips (the common case).**
  When `clonepath/org/repo` is already a flat checkout (not yet migrated),
  `clone org/repo` keeps today's behavior: update (auto-stash + pull) and cd into
  the flat checkout, then print a one-line hint suggesting `clone --migrate` to
  convert it. It does **not** silently convert and does **not** refuse. This is
  what keeps the hundreds of existing flat clones usable during the mixed-
  ecosystem rollout.
- **Empty / commitless repo.** If origin has no branches (freshly created repo),
  build the container but skip the worktree add (would fail with no commits);
  warn and cd into the container. Ties into the existing empty-repo recovery.
- **Flag conflicts.** `--versioning` implies `--flat` (versioned `repo/<sha>`
  checkouts are incompatible with bare worktrees); `--flat` with `--worktree` or
  `--migrate` is rejected by `Config::try_from`. `--migrate` requires a clean,
  stash-free tree (see migrate sequence).
- **Mirror reference.** `--mirrorpath` (`git clone --reference`) composes with
  `--bare` unchanged.

### Rollout Plan
- Ship behind the layout default flip: new clones are bare; `--flat` restores
  old behavior; existing flat clones are untouched until `--migrate`d. Mixed
  ecosystem is expected and supported by discovery.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `--bare` empty fetch refspec leaves remotes untracked | High | High | Mandatory refspec fix + fetch in setup (proven); covered by test |
| Worktree placed outside org prefix → wrong persona identity | Med | High | Enforce under-`~/repos/<org>/` placement by construction; assert in setup |
| Tooling assumes `~/repos/<org>/<repo>` is a working tree | High | Med | `cd` lands in the default worktree (always present); the `cd`/`z` shim redirects other navigation; discovery reports the logical repo; document the shape change |
| Migrate loses unpushed commits on a clean-but-ahead branch | Med | High | Clone the bare from the **local** repo (not `origin`) so all local refs/commits are captured (proven); non-destructive rename-aside swap |
| `git clean -xfd` deletes something in bare mode | Low | High | Bare container is never checked out; clean logic removed from bare path |
| Discovery multiplies remote calls across worktrees | Med | Med | One `RepoInfo` per logical repo by default; per-worktree rows are opt-in only |
| Worktree subprocess failures are opaque (branch in use, path exists) | Med | Med | Route through the shared `common::git::run` (captures stderr into the error context); surface on stderr, keep stdout pure |

## Open Questions
- [x] **Resolved.** Worktree dir name = the branch slug. New branches are
      slugified to lowercase-hyphenated (no `feature/` prefixes) and used as both
      branch and dir; existing non-slug branches keep their real name and get a
      slugified dir only. `--worktree`'s argument is slugified for new-branch
      lookups.
- [x] **Resolved.** Repo-path contract = container is the logical repo; the
      always-present default-branch worktree is the canonical working tree; a
      `cd`/`z` shim redirects navigation into it. Discovery emits one logical-repo
      row; per-worktree is opt-in.
- [ ] Should `clone.cfg` gain `default-layout: bare|flat`, or is the `--flat`
      flag + hardcoded bare default enough? Proposed: add the config key (CLI
      overrides it) for per-machine control.
- [ ] Exact form of the `cd`/`z` shim (a `cd` wrapper function vs a zoxide hook),
      and whether it ships in `shell-functions.sh` here or alongside the user's
      existing zoxide config.

## References
- `2026-06-21-common-shared-infra.md` — the consolidated parser/runner/discovery
  primitives this design depends on and extends.
- Prototype sessions (2026-06-21): bare setup + refspec gotcha + persona-prefix
  verification; rebase/merge-across-worktrees; migrate-from-local vs from-origin
  data-loss proof; `.git`-pointer fail-closed check.
- Architect (Gemini) + Staff Engineer (Codex) design reviews (2026-06-21):
  migrate data-loss, slug-lookup ordering, discovery blast radius, rerun
  semantics, stderr capture, persona-invariant test.
- `git-tools/CLAUDE.md` — wrapper contract (v0.2.5 stdout bugfix), install wiring.
- `~/.gitconfig` `includeIf "gitdir:~/repos/tatari-tv/"`; `~/.config/clone/clone.cfg`.
- The Modern Coder, "Worktrees missing piece" (bare-repo model);
  `second-brain` vault notes on git worktrees for agentic AI.
