# Design Document: Flat-by-default clone + worktree fleet hygiene

**Author:** Scott A. Idler
**Date:** 2026-07-03
**Status:** Implemented (git-tools phases 0a/1/2/3/4, gx phases 0b + v0.3.1; git-tools Phase 4 audit remediation lands in the next patch release)
**Review Passes Completed:** 5/5 + three cross-model panels (design-research; Architect/Gemini + Staff-Engineer/Codex design review; second focused panel on the `--flatten` contract; post-implementation audit 2026-07-04 across git-tools + gx, which produced Phase 4)

## Summary

`clone` currently stamps out a **bare-container + worktrees** layout on *every*
clone. That layout only earns its complexity for the handful of repos where
multiple AI agents work branches in parallel; for everything else it is pure
tax. This doc makes **flat the default** and bare an explicit opt-in
(`clone --bare`), adds a **safe, refuse-first way to collapse** gratuitous bare
containers back to flat, and teaches the one bulk tool that is not layout-aware
(`gx`) about bare containers. `.worktreeinclude` seeding was considered and
**cut** — the cross-model review panel unanimously found it does not pay off for
this fleet.

## Problem Statement

### Background

`clone` was changed (docs: `2026-06-21-clone-bare-worktree.md`) so a normal
`clone <org>/<repo>` produces:

```
<org>/<repo>/
  .bare/          # bare git db (shared object store)
  .git            # pointer file: "gitdir: ./.bare"
  main/           # worktree for the default branch
  <branch>/       # one worktree dir per added branch
```

The container **root is not a working tree**. `git status` at the root fails
with `fatal: this operation must be run in a work tree`. A `worktree` CLI
(`2026-06-28-worktree-tool.md`) adds/lists/prunes worktrees in this layout, and
`clone --migrate` converts a flat checkout into a bare container.

The decision to make bare the *default* is the thing this doc revisits. It was
based on the parallel-AI-agent workflow, which is real but narrow.

### Problem

A filesystem survey of `~/repos/{tatari-tv,scottidler}` found **~300 flat
checkouts and 14 bare containers**. Of the 14, only a handful have more than the
`main/` worktree (marquee: 9, loopr: 3, second-brain: 3, clyde: 3); the rest are
single-`main` containers that gained nothing from the bare layout but inherited
all of its costs:

1. **Every future clone is bare.** The default keeps minting containers that
   will only ever hold `main/`, so the gratuitous fraction grows over time.
2. **The root gotcha breaks tools and muscle memory.** `cd <repo> && git ...`
   fails at a container root. Any bulk operation that iterates repo directories
   and runs git at the root errors or silently skips. `gx` — a multi-repo git
   tool — is exactly such a consumer and does not understand the layout today.
3. **The fleet is split with no way back.** `clone --migrate` is strictly
   flat→bare (`clone/src/migrate.rs:84`, bails if already bare). There is no
   supported bare→flat collapse, so the 14 accidental containers are stuck.

### Goals

- **Flat is the default.** `clone <org>/<repo>` produces a flat checkout; bare
  requires an explicit `--bare` opt-in.
- **A safe, refuse-first bare→flat collapse** so accidental containers can be
  cleaned up without hand-`rm` and without losing any ref-reachable work.
- **`gx` handles both layouts** so bulk ops don't break on the bare containers
  that legitimately remain — treating a bare container as **one logical repo**
  (its default worktree), not N repos.

### Non-Goals

- **`.worktreeinclude` / seeding gitignored files.** Considered and cut (see
  Alternatives). `target/` is already relocated by `relocate-targets` and must
  not be seeded; `.env` is rare in this fleet; the seeder would have almost
  nothing to seed while adding a secret-sprawl surface.
- **Per-worktree runtime state isolation** (separate DBs, ports).
- **Process / root / network isolation** (containers, VMs).
- **The verification / merge-gate layer** — the real throughput bottleneck once
  agents parallelize, but a separate effort.
- **Automatic mass migration** in either direction. Collapsing a container is an
  explicit, per-repo, human-initiated action.
- **Changing the `worktree` tool's prune/list/switch semantics.**

### Usecase context (why the cut decisions are right for this fleet)

- The fleet is overwhelmingly **small Rust CLIs**; the dominant gitignored
  artifact is **`target/`**, already relocated to a dedicated SSD via
  `relocate-targets` (a symlink `target -> /media/.../target`). Seeding it would
  fight that tool.
- In-repo secrets/`.env` are **rare** (config lives under XDG
  `~/.config/<proj>/`), unlike the Node/Python examples that motivate
  `.worktreeinclude` in the wild.
- Parallel agents run on **~4 repos** (loopr, marquee, second-brain, clyde) via
  `herdr`/`loopr`, not fleet-wide.
- Config precedence in `clone` today is **CLI flag > `clone.cfg` >
  built-in default** — there is no environment-variable layer in
  `resolve_layout` (`clone/src/config.rs`). `clone.cfg` already has a
  `[clone] default-layout` key.

## Proposed Solution

### Overview

Three independently-committable phases, ordered so nothing breaks mid-transition:

0. Make `gx` layout-aware (unblocks bulk ops on surviving containers).
1. Flip `clone`'s default to flat; add a `--bare` opt-in.
2. Build a safe, refuse-first bare→flat collapse (`clone --flatten`).
3. E2E lifecycle tests.

Phase 0 comes first because bare containers legitimately remain after cleanup, so
`gx` must handle them *before* we start collapsing/keeping the mixed fleet.

### Architecture

- **`common` crate** — already classifies bare-vs-flat (`repo/discovery.rs:187`,
  `is_bare_container` at `bare.rs:17`) and has **four** `git worktree list
  --porcelain` parsers today (`repo/info.rs:63`, `bare.rs:212`,
  `clone/src/migrate.rs:450`, `worktree/src/list.rs:21`). Phase 0a unifies these
  into one `resolve_worktrees()`.
- **`clone`** — `resolve_layout()` (`config.rs:114`) default arm flips to Flat;
  new `--bare` flag; new `--flatten` reverse migration with a purpose-built,
  refuse-first preflight (it does **not** reuse `rescue_work`/`check_connectivity`
  — see below).
- **`gx`** — the sole non-layout-aware consumer. Fix repo discovery
  (`gx/src/repo.rs:101,151,196`), slug/origin extraction (`:23,209`), **and the
  additional `.git`-as-directory assumptions in `gx/src/cleanup.rs:224` and
  `gx/src/rollback.rs:15`** flagged by the panel. gx gets a **vendored** minimal
  bare-detection module (not a cross-repo dependency on `common`).
- **`ls-*` family** — already layout-aware via `common::repo::RepoDiscovery`; no
  change required.

### Data Model

Unified worktree row (Phase 0a), replacing the four ad-hoc parsers. Fields are
the union of what all four callers need — the panel caught that a minimal
`{path, branch}` row would break migrate's detached-HEAD rescue (needs `head`)
and prune/list (need `locked`):

```rust
// common::bare
pub struct WorktreeRow {
    pub path: PathBuf,
    pub branch: Option<String>, // None = detached HEAD
    pub head: Option<String>,   // commit sha; migrate's detached rescue needs it
    pub bare: bool,             // the .bare entry itself
    pub locked: bool,           // prune/list need it
}
pub fn resolve_worktrees(container: &Path) -> Result<Vec<WorktreeRow>>;
```

The second review pass confirmed these five fields are sufficient for all four
call sites (no caller consumes git's porcelain `prunable`/lock-reason strings —
`prune.rs:55` computes its own decision from `branch`+`locked`). **`WorktreeRow`
is not the flatten safety model**, though: bisect/rebase/cherry-pick, per-worktree
config, and sparse-checkout are not row fields, so Phase 2 must inspect each
worktree's private `.bare/worktrees/<id>/` gitdir directly (see the `--flatten`
contract).

### API Design

`clone` CLI after the change:

```
clone <org>/<repo>              # FLAT checkout (new default)
clone <org>/<repo> --bare       # bare container + worktrees (opt-in)
clone --migrate                 # flat -> bare (unchanged)
clone --flatten                 # bare -> flat (NEW; refuses on unsafe state)
clone --flatten --dry-run       # preview collapse (retained refs, removals)
```

- `--flat` is retained as a redundant no-op alias for one release, then dropped
  from docs. `--versioning` continues to imply flat.
- `[clone] default-layout` in `clone.cfg` still overrides the built-in default,
  so bare-by-default remains available to anyone who sets it explicitly.

#### `--flatten` safety contract (rewritten across two review passes)

A first refuse-first draft was reviewed and found insufficient: its retention
proof (diff `refs/heads/*` + `refs/tags/*` before/after) was theater — those
refs live in the shared `.bare/refs/` and survive the `.bare`→`.git` move for
free, so the diff proves nothing about the per-worktree state actually at risk,
nor about object-graph integrity in the new store. Real state escaped every
check: a detached HEAD in a non-default worktree (orphaned on removal), an
existing `refs/stash` (which forward migrate deliberately rescues to `wip/*`,
`migrate.rs:551`), an active bisect/rebase/cherry-pick/revert, and
`refs/notes|replace|*` / custom refs / per-worktree config / sparse-checkout /
submodule state.

> **Invariant (final): the collapse preserves every ref under `refs/*` at an
> identical OID, AND any per-worktree state not represented by a preserved ref
> blocks the collapse.**

Preflight is refuse-first and purpose-built — it does **not** reuse
`rescue_work`'s auto-stash or `check_connectivity` (flatten is local; no remote
needed). Enumerate worktrees via `resolve_worktrees()`, then **additionally
inspect each worktree's private `.bare/worktrees/<id>/` gitdir** (these states
are not `WorktreeRow` fields). **Refuse** if any worktree has:

- uncommitted changes or untracked files;
- a local branch that is not an ancestor of the default (ancestry, not
  `diff-filter=U`);
- a detached HEAD whose commit is not reachable from a preserved ref;
- an existing `refs/stash`;
- an active merge / rebase / cherry-pick / revert / bisect — detected via
  `MERGE_HEAD`, `rebase-merge/`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`,
  `BISECT_START` in the worktree's gitdir;
- per-worktree config or sparse-checkout state, or dirty submodules.

**Ignored files are local user data, never silently dropped.** During
`--dry-run` and preflight, run `git status --porcelain --ignored=traditional` in
every worktree. Because the crash-safe transition below archives the *entire*
original container via `rkvr` before the swap, any non-default worktree's ignored
files (`.env`, build state, `target` symlink) remain recoverable from that
archive; report their paths as recoverable. There is no report-only path that
removes a worktree with ignored files without that recoverable archival.

**Retention proof (replaces the heads/tags diff):** enumerate all refs under
`refs/*` before and after and assert identical OIDs; run
`git fsck --connectivity-only` on the staged flat DB; and
`git cat-file -e <oid>` every pre-transition ref OID against the staged store.

**Crash-safe DB transition (copy-based — keeps the live `.bare` intact until
swap+verify):**

1. Copy the live `.bare/` to `<repo>.flattening/.git/` (objects, refs, hooks,
   config, reflogs). Never mutate the live container in place.
2. In the staged `.git/config`: `core.bare=false`, unset `core.worktree`,
   `core.logAllRefUpdates=true`; leave `remote.origin.fetch` as
   `+refs/heads/*:refs/remotes/origin/*` (the container already uses the
   non-bare refspec, `bare.rs:86`; do NOT introduce `+refs/heads/*:refs/heads/*`
   — it can clobber local branches on the next fetch).
3. Remove the staged `.git/worktrees/` admin entries (safe only because preflight
   refused all non-discardable per-worktree state; stale entries otherwise make
   git think branches are checked out elsewhere).
4. Materialize: `GIT_DIR=<staging>/.git GIT_WORK_TREE=<staging> git reset --hard
   <default>`.
5. Verify: staged `git status` clean, all pre-transition ref OIDs present,
   `git fsck --connectivity-only` passes.
6. Atomic swap: rename `<repo>`→`<repo>.flatten-backup`, then
   `<repo>.flattening`→`<repo>`; re-verify at the final path.
7. Only after final verification, `rkvr rmrf <repo>.flatten-backup`.
8. On post-swap failure: remove the staged final and rename the backup back.

**Refuse-first is a dead-end only until the user cleans up.** A dirty/unpushed
worktree cannot collapse until committed/pushed/pruned — the right default for a
data-loss-prone structural op. Document the remediation path (`worktree prune`,
commit/push) rather than adding auto-rescue; that auto-stash is exactly what made
the first draft self-contradictory.

### Implementation Plan

#### Phase 0a: Unify worktree enumeration in `common`
**Model:** sonnet
- Add `common::bare::resolve_worktrees() -> Vec<WorktreeRow>` (fields above) with
  one porcelain parser.
- Refactor all **four** existing parsers (`repo/info.rs:63`, `bare.rs:212`,
  `migrate.rs:450`, `worktree/src/list.rs:21`) to call it.
- Unit tests in `common/src/bare/tests.rs` (TempDir container fixtures,
  including detached-HEAD and locked worktrees).

#### Phase 0b: Teach `gx` both layouts (vendored module)
**Model:** opus
- Vendor a minimal (~50 line) bare-detection module into gx: strict container
  detection (`.git` pointer file + `.bare/` dir), one porcelain parser, default-
  worktree resolution, origin via `git remote get-url origin`. **No cross-repo
  `path`/`git` dependency on `common`** (gx is a separate repo; coupling its
  build to git-tools' unreleased internal API was rejected by both reviewers).
- **Semantics: a bare container is ONE logical repo = its default worktree**
  (`per_worktree = false` equivalent), so write commands don't fan out N× over a
  single repo.
- Fix every `.git`-as-directory assumption: `repo.rs:101,151,196`, slug/origin at
  `:23,209`, **and `cleanup.rs:224`, `rollback.rs:15`**.
- gx integration tests with a bare container in the scan set (assert it counts as
  one repo and git runs at the default worktree).

#### Phase 1: Flip `clone` default to flat, add `--bare`
**Model:** sonnet
- Flip the default arm of `resolve_layout()` (`config.rs:114`) to `Flat`; update
  the `test_resolve_layout_default_is_bare` test accordingly.
- Add `--bare` to `cli.rs:37`; reconcile with `--flat`, `--versioning`, and the
  `default-layout` cfg key (precedence: CLI > cfg > default; no env layer).
- Update `after_help` and the `clone` skill doc.

#### Phase 2: Safe bare→flat collapse (`clone --flatten`)
**Model:** opus
- Lift the genuinely reusable, non-network migrate helpers into `common`
  (`list_worktrees`→`resolve_worktrees`, `is_dirty`, `require_rkvr`,
  `assert_all_clean`). Do **not** lift/reuse `check_connectivity`; do **not**
  reuse `rescue_work`'s auto-stash for the structural collapse.
- Add per-worktree gitdir inspection (`.bare/worktrees/<id>/` state files) — the
  refuse checks the `WorktreeRow` cannot carry.
- Implement `--flatten` + `--dry-run` honoring the refuse-first contract, the
  `refs/*`-identical-OID invariant, the ignored-file archival rule, the
  fsck+cat-file retention proof, and the copy-based crash-safe DB transition
  above.
- Tests: clean single-`main` container (collapses); merged feature worktrees
  (collapses, all `refs/*` OIDs identical after); dirty/untracked worktree
  (refuses); unpushed-but-clean local branch (refuses — the case the old
  `has_unmerged` missed); detached HEAD in a non-default worktree (refuses);
  existing `refs/stash` (refuses); active bisect/rebase (refuses); ignored file
  in a removed worktree (recoverable from the rkvr archive, reported);
  mid-transition crash leaves the live `.bare` intact (copy-not-move);
  `git fsck --connectivity-only` on the staged DB; refspec unchanged after
  collapse.

#### Phase 3: E2E lifecycle tests
**Model:** sonnet
- `clone/tests/` + gx integration: flat-default, `--bare`, forward migration,
  reverse `--flatten` round-trip, and gx treating a container as one repo.

#### Phase 4: Post-implementation audit remediation
**Model:** opus
Findings from the two-model implementation audit (2026-07-04, Architect/Gemini +
Staff-Engineer/Codex, git-tools + gx). gx's finding already shipped as v0.3.1;
the git-tools items below are pending (target: git-tools v0.3.2).
- **[HIGH, git-tools] Fail-closed preflight.** `clone --flatten`'s preflight
  (`clone/src/flatten.rs::inspect`) currently warns-and-continues when a safety
  check itself ERRORS: `is_dirty` Err (~:222), `worktree_gitdir` Err (~:245,
  skipping both in-progress-op and per-worktree-config/sparse checks), and
  `dirty_submodules` treating a non-success `git submodule status` as clean
  (~:405) plus its warn-only Err arm (~:255). This violates the refuse-first
  invariant and the "defaults fail CLOSED" rule: an undeterminable check must
  BLOCK the collapse, not proceed. Fix: those Err/non-success arms push a
  refusal. (`reset --hard` can otherwise discard a worktree's uncommitted work
  while `verify_flat` still passes, since the retention proof guards only
  `refs/*`, not working-tree state; only the rkvr archive saves it.)
- **[MEDIUM, git-tools] Phase 2/3 test hardening.** Add: a dirty-submodule
  refuse test; a real in-progress rebase/cherry-pick/revert refuse test (not
  only the synthetic MERGE_HEAD/bisect); an assertion that ignored-file
  reporting reaches stderr; and a binary-level migrate->flatten round-trip that
  asserts a local-only `refs/*` entry survives (current e2e seeds via `--bare`,
  not `--migrate`, so it would not catch a dropped local ref).
- **[LOW, git-tools] Doc drift.** `CLAUDE.md` (~:105,130,151) still documents
  bare as `clone`'s default; update to flat-default + `--bare`.
- **[already shipped, gx v0.3.1] `gx clone` bare-container update.**
  `clone_or_update_repo` ran git status/checkout/pull at the container root
  (not a work tree, exit 128); now routes through `bare::default_worktree`
  (`gx/src/git.rs::resolve_update_work_tree`) with a regression test.
- **Success criteria:** a `--flatten` preflight where `is_dirty`/`worktree_gitdir`/
  `dirty_submodules` errors REFUSES (new test); `otto ci` green in git-tools;
  `CLAUDE.md` no longer says bare-default; gx v0.3.1 regression test present.

## Alternatives Considered

### Alternative 1: Build `.worktreeinclude` seeding (original Phase 3)
- **Description:** A declarative manifest of gitignored paths (e.g. `.env`) copied
  or symlinked into each new worktree on creation.
- **Pros:** Solves the #1 pain in the wild — fresh worktrees lack `.env` /
  `node_modules` / build state.
- **Cons:** For *this* fleet there is almost nothing to seed: `target/` is owned
  by `relocate-targets` and must not be touched; `.env`/secrets are rare (XDG
  config); only ~4 repos parallelize. It adds cross-worktree file-sync machinery
  and a secret-sprawl surface for near-zero benefit.
- **Why not chosen:** **Both review-panel models independently said drop it.** It
  is a Node/Python-workflow import that does not pay off here. If one of the ~4
  agent repos ever hits bootstrap pain, solve it locally in that repo.

### Alternative 2: gx depends on git-tools' `common` crate
- **Description:** Add `common` as a `path`/`git` dependency of gx (as `ls-*` do).
- **Pros:** Single source of truth; no duplicated logic.
- **Cons:** Couples gx's build to git-tools' local checkout topology and to an
  unreleased internal API; gx is a separate repo, not a workspace member.
- **Why not chosen:** Both reviewers rejected it. Vendor ~50 lines instead;
  extract a published crate later only if a third consumer appears.

### Alternative 3: Keep bare as the default, just make tools cope
- **Description:** Leave `clone` defaulting to bare; only make consumers layout-
  aware.
- **Cons:** Every consumer forever pays the two-layout tax; the gratuitous-
  container fraction keeps growing; the root gotcha stays a daily papercut for
  ~300 repos that never needed it.
- **Why not chosen:** Optimizes for the ~5% workflow at the 95%'s expense.

### Alternative 4: Migrate the whole fleet one way
- **Description:** Convert all 314 repos to a single layout.
- **Cons:** All-bare re-imposes the tax universally; all-flat removes the layout
  from the repos that genuinely use parallel agents.
- **Why not chosen:** The fleet has two real workloads; the unit of choice is the
  repo, not the fleet.

## Technical Considerations

### Dependencies
- No cross-repo dependency added. gx vendors a minimal module. No new external
  crates anticipated.

### Performance
- `resolve_worktrees()` is one `git worktree list --porcelain` per container —
  same cost as today, deduplicated across four call sites.

### Security
- `--flatten` removals honor `require_rkvr` (archival delete, never `rm -rf`).
- Dropping `.worktreeinclude` removes a would-be secret-sprawl surface.

### Testing Strategy
- Unit tests per changed module (`src/<mod>/tests.rs`); integration tests spawn
  the built binary (`clone/tests/integration_tests.rs` pattern) over TempDir
  workspaces. Full `otto ci` (`cargo test --workspace --all-features`, clippy
  `-D warnings`, `shell-test`) green before each phase commits. gx has its own
  `otto ci`.

### Rollout Plan
- Land Phase 0 first (gx layout-aware) so the mixed fleet is safe. Then Phase 1
  (default flip). Then collapse accidental containers with Phase 2 as they are
  encountered — no batch job.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| `--flatten` loses unpushed/uncommitted/per-worktree state | Low | High | Refuse-first; `refs/*`-identical-OID invariant + per-worktree gitdir inspection; refuse on detached-HEAD/stash/bisect/rebase/cherry-pick/sparse-checkout/dirty-submodule; ancestry (not `diff-filter=U`) merge check; whole-container rkvr archive so ignored files stay recoverable; fsck + `cat-file -e` proof; copy-then-atomic-swap; `--dry-run` |
| gx vendored module drifts from `common` | Med | Low | ~50 lines, well-tested; extract a shared published crate if a third consumer appears |
| Default flip surprises muscle memory | Med | Low | `--bare` + `default-layout` cfg escape hatch; announce in skill doc |
| Phase 0a misses a parser / field | Low | Med | Panel confirmed four parsers; `WorktreeRow` carries `head` + `locked` for migrate/prune |
| `--flatten` DB transition botched | Low | High | Transition (`.bare`→`.git`, clear `core.bare`) specified explicitly + verified in tests |

## Open Questions

- [ ] **`is_bare_container` breadth:** it currently accepts any `.bare/` dir, not
  specifically the `.git`-pointer + `.bare/` pair. Decide in Phase 0b what
  precisely counts as a container (the vendored gx detector should be strict).
- [ ] **`--bare` alias:** ship `--agents` as an alias now, or keep the flag purely
  a layout choice? Low stakes; default is `--bare` only.

## References
- `docs/design/2026-06-21-clone-bare-worktree.md` — the bare layout introduction
- `docs/design/2026-06-28-worktree-tool.md` — the `worktree` CLI
- `docs/design/2026-06-28-consolidate-worktree-primitives.md` — `common::bare`
- `docs/design/2026-06-21-common-shared-infra.md` — the shared crate
- Review panel (2026-07-03): Architect/Gemini + Staff-Engineer/Codex, Design
  Review mode — unanimous on cutting `.worktreeinclude`, vendoring gx's module,
  and rewriting the `--flatten` contract.
- Vault notes: `worktrees-missing-piece.md`, `how-agents-quietly-break-architecture.md`,
  `how-this-ex-meta-l8-engineer-ships-40-prs-a-day-...-kun-chen.md`
