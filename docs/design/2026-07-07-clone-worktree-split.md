# Design Document: clone/worktree architectural split

**Author:** Scott Idler
**Date:** 2026-07-07
**Status:** Implemented
**Review Passes Completed:** 5/5

## Summary

Split acquisition from layout across the two git-tools binaries. `clone` becomes
pure acquisition: resolve repospec, pick SSH key, fetch, always flat. `worktree`
becomes the sole owner of the bare-container lifecycle: a new `worktree init`
verb for fresh bare acquisition, plus `migrate` (flat->bare) and `flatten`
(bare->flat) moved off `clone`. Shared transport and config reading move to
`common`. This is a breaking CLI change: `clone --bare/--migrate/--flatten`
disappear.

## Problem Statement

### Background

- `clone org/repo` produces a flat checkout by default (v0.3.x). `--bare` opts
  into a bare-container + worktree layout. `--migrate` converts flat->bare in
  place; `--flatten` converts bare->flat in place. Per-machine default via
  `[clone] default-layout` in `clone.cfg`.
- `worktree` (separate binary) owns day-2 worktree lifecycle inside an
  already-existing bare container: add/switch (positional branch), list, prune,
  pick. It is purely local today: no network, no transport dependency.
- History: bare-by-default was tried, then reverted to flat-default after a
  fleet survey (~300 flat checkouts vs 14 bare, most of the 14 gratuitous).
  Bare is a minority layout for the handful of repos where agents work branches
  in parallel. Docs: `docs/design/2026-06-21-clone-bare-worktree.md`,
  `docs/design/2026-06-28-worktree-tool.md`,
  `docs/design/2026-06-28-consolidate-worktree-primitives.md`,
  `docs/design/2026-07-03-clone-flat-default.md`.

### Problem

Three concerns that change for unrelated reasons are coupled inside the `clone`
crate:

- **Transport / acquisition** (network, SSH, URL, fetch): `clone/src/transport.rs`,
  the flat-clone path in `clone/src/lib.rs`, the SSH-key reader in
  `clone/src/config.rs`.
- **Bare-layout construction** (worktree add, refspec fixup, `.git` pointer):
  `clone/src/bare.rs`.
- **Pure-local layout conversion, zero network I/O**: `clone/src/migrate.rs`
  (836 lines), `clone/src/flatten.rs` (656 lines).

`--migrate` and `--flatten` do no cloning (verified: `clone/src/lib.rs:34-62`
dispatches both to local `migrate::`/`flatten::`; `migrate` clones *from the
local repo* at `migrate.rs:137` and only touches the network read-only;
`flatten` never hits the network). A tool named `clone` performing conversions
that clone nothing is the cognitive-dissonance smell `taste.md` forbids
("names tell the truth"). Transport and layout are orthogonal, coupled only by
accident of where the flags landed.

### Goals

- `clone` owns acquisition only: repospec -> SSH key -> fetch -> flat checkout.
- `worktree` owns the entire bare-container lifecycle: `init` (fresh
  acquisition), `migrate`, `flatten`, plus existing add/switch/list/prune/pick.
- Transport and the shared config reader live in `common`; neither binary
  depends on the other's crate.
- Decompose along change frequency: transport code and layout code change for
  unrelated reasons and stop being coupled.
- Match fleet usage: the majority-use tool (`clone`, ~95% of repos) carries no
  bare-layout code; the minority-use tool (`worktree`) carries all of it.

### Non-Goals

- Merging the two binaries into one. The git-tools fleet is one-binary-per-verb
  (`ls-git-repos`, `ls-github-repos`, `reposlug`, etc.); this preserves that.
- A `clone --bare` deprecation shim. It becomes a hard unknown-argument error.
- Preserving a per-machine "always bare" default. `default-layout` is removed;
  bare is always an explicit `worktree init` / `worktree migrate`.
- Changing the worktree add/switch/list/prune/pick behavior. Those are
  untouched except for the new verbs slotting alongside them.
- Removing the old `clone.cfg` INI fallback path. It stays as a back-compat
  read; its eventual removal is parked (see Addendum).

## Proposed Solution

### Overview

- Extract transport and the config reader from `clone` into `common`.
- Move `bare`/`migrate`/`flatten` (code + tests) from `clone` into the
  `worktree` crate.
- Add three reserved-word positional verbs to `worktree`: `init <spec>`,
  `migrate [spec]`, `flatten [spec]`, dispatched pre-clap (mirroring the
  existing `shell-init` interception).
- Strip `--bare/--flat/--migrate/--flatten/--dry-run`, the `Layout` enum,
  `run_bare`, and `resolve_layout` from `clone`.
- Migrate config to YAML at a shared XDG path, drop `default-layout`, keep an
  INI fallback read of the old `clone.cfg` path.

### Architecture

```
common/
  transport.rs   # NEW: clone_with_fallback, try_clone, REMOTE_URLS
  config.rs      # NEW: shared reader (git-tools.yml YAML, clone.cfg INI fallback)
  bare.rs        # unchanged shared primitives (add_worktree, resolve_and_add, ...)

clone/           # acquisition only, always flat
  lib.rs         # run_clone + flat paths; no run_bare, no Layout
  cli.rs         # no --bare/--flat/--migrate/--flatten/--dry-run
  transport.rs   # DELETED (moved to common)

worktree/        # entire bare-container lifecycle
  bare.rs        # MOVED from clone
  migrate.rs     # MOVED from clone
  flatten.rs     # MOVED from clone
  init.rs        # NEW: fresh bare acquisition (common::transport + bare)
  cli.rs / config.rs  # Op::Init/Migrate/Flatten added; pre-clap dispatch
```

Dependency direction after the split: `clone -> common`, `worktree -> common`.
No `clone <-> worktree` edge.

### Data Model

**Shared config (`common::config`), YAML at `~/.config/git-tools/git-tools.yml`:**

```yaml
default-branch: main            # fallback default branch (was [clone] default)
orgs:
  tatari-tv:
    sshkey: ~/.ssh/tatari
  default:
    sshkey: ~/.ssh/id_ed25519
```

- Deserialized with serde (`rename_all = "kebab-case"`, `deny_unknown_fields`).
- `default-layout` is NOT carried into the YAML schema (removed).
- Reader tries these locations in order; the first file that exists wins, and
  its **format is determined by the path** (new paths = YAML, old paths = INI).
  The reader logs which file it loaded.

  | Order | Path | Format |
  |-------|------|--------|
  | 1 | `$GIT_TOOLS_CFG` (new explicit override) | YAML |
  | 2 | `~/.config/git-tools/git-tools.yml` (XDG, honors `$XDG_CONFIG_HOME`) | YAML |
  | 3 | `$CLONE_CFG` (old explicit override, back-compat) | INI |
  | 4 | `~/.config/clone/clone.cfg` (old default, back-compat) | INI |

- Both formats deserialize into the same in-memory struct (`default-branch` +
  per-org sshkeys), so callers are format-agnostic.
- XDG resolution via the house `xdg_config_dir()` helper, never
  `dirs::config_dir()`.
- **Fail-closed load semantics** (resolving review finding 6; today's readers are
  inconsistent: `clone_cfg_value` returns `None` on a missing file/key while
  `find_ssh_key_for_org` errors when a present config lacks `[org.*]`):
  - The FIRST location whose file exists is THE config. If it exists but fails to
    parse (malformed YAML/INI), that is a loud error; the reader does NOT silently
    fall through to a lower-precedence file (a partial/empty higher file must not
    be shadowed by an old one). Fall-through happens only when a file is ABSENT.
  - Missing `default-branch`: fall back to the built-in default branch (a lookup
    default, not an error).
  - Missing `orgs` / no match for an org / no `default` org: SSH-key lookup
    returns `None` (transport proceeds with no `-i` key), never a hard error.
    This unifies the two current behaviors on the permissive-for-lookup side
    while keeping parse failures loud.
  - Unknown YAML fields are rejected (`deny_unknown_fields`): a typo is a loud
    error, not silent data loss.

**worktree `Op` (extends `worktree/src/config.rs:8-18`):**

```rust
enum Op {
    Pick,                       // existing
    List,                       // existing
    Prune,                      // existing
    Switch(String),             // existing
    Init(RepoSpec),             // NEW: fresh bare acquisition
    Migrate(Option<RepoSpec>),  // NEW: flat -> bare
    Flatten(Option<RepoSpec>),  // NEW: bare -> flat
}
```

`dry_run: bool` added for `migrate`/`flatten` preview (matches clone's current
`--flatten --dry-run`).

### API Design

**clone (after):**

```
clone <spec>                 # flat checkout, cd into it (unchanged)
clone --versioning <spec>    # flat, timestamped mirror (unchanged)
clone --flat <spec>          # retained no-op alias for the default (unchanged)
clone --bare|--migrate|--flatten   # ERROR: unknown argument
```

**worktree (after):**

```
worktree <branch>            # switch-or-create worktree (unchanged, cd)
worktree                     # picker (unchanged)
worktree --list | --prune    # unchanged (no cd)
worktree init <spec>         # NEW: fresh bare container, cd into default worktree
worktree init --clonepath D --remote URL <spec>   # overrides (defaults: "." and REMOTE_URLS[0])
worktree migrate [spec]      # NEW: flat -> bare in place, cd
worktree flatten [spec]      # NEW: bare -> flat in place, cd
worktree migrate|flatten --dry-run   # preview to stderr, empty stdout, wrapper does NOT cd
```

**Wrapper contract (unchanged invariant):** binary prints destination path to
stdout only, all errors to stderr, non-zero exit on failure; the shell function
captures stdout, bails before any `cd` on non-zero exit. The new verbs must land
in the `worktree()` wrapper's capture-and-`cd` branch. Because the current
wrapper passes any `-*` first arg straight through with no `cd`
(`worktree/src/shell.rs:33-48`), the verbs are **positionals**, not flags, and
are dispatched pre-clap in `worktree/src/main.rs` (mirroring `shell-init` at
`main.rs:14`) so clap never mistakes them for the positional `branch`.

### Implementation Plan

#### Phase 1: Extract transport + config reader to `common`
**Model:** sonnet
- Create `common/src/transport.rs`: move `clone_with_fallback`, `try_clone`
  (`clone/src/transport.rs:17-97`) and `REMOTE_URLS` (`clone/src/lib.rs:21`).
- Create `common/src/config.rs` reader: move `find_ssh_key_for_org` and
  `clone_cfg_value` (`clone/src/config.rs:181-224`) verbatim (still INI, still
  reading the old paths). `cargo add ini` to `common`.
- Rewire clone call sites: `lib.rs:206` (flat), `bare.rs:39` (bare),
  `migrate.rs:403` -> `common::transport` / `common::config`.
- Behavior-neutral: `clone` and `clone --bare` behave byte-identically.
- **Success criteria:** `otto ci` green; `clone org/repo` and `clone --bare org/repo`
  produce identical layouts to pre-phase; no transport/`ini` symbol defined in
  `clone` that isn't re-exported from `common`.

#### Phase 2: Relocate bare/migrate/flatten into worktree + add verbs
**Model:** opus

**Adapt, do not copy verbatim** (closes review findings 1, 2, 5). Verified
coupling: `clone/src/bare.rs:17,28` take `&clone::config::Config` and read
`config.{clonepath,remote,mirrorpath,ssh_key,verbose}`; `worktree::Config` has
none of those, and `worktree/src/bare.rs` already exists (owns
`resolve_container_from_cwd`, re-exports `common::bare`). A verbatim copy fails to
compile (type mismatch) and collides on the module name. So:

- Introduce a worktree-local scalar args struct
  `AcquireArgs { clonepath, remote, mirrorpath, ssh_key, verbose }` and fold the
  relocated `setup_bare_container`/`reconcile_container`/`fix_fetch_refspec`/
  `ensure_default_worktree` into the EXISTING `worktree::bare` module, taking
  `&AcquireArgs` (never a clone-shaped `Config`).
- Relocate `migrate`/`flatten` the same way (they also take a clone `Config`;
  adapt to scalar args / `worktree::Config`). Their cwd resolvers
  (`migrate.rs::flat_from_cwd`, `flatten.rs::container_from_cwd`) move too.
- Add `worktree/src/init.rs`: fresh bare acquisition via
  `common::transport::clone_with_fallback` + `common::bare` primitives + the
  relocated refspec fixup; spec parsed with `common::git::parse_repospec`.

**Split dispatch so the new verbs don't require an enclosing container**
(closes finding 2). Today `worktree::run` calls `resolve_container_from_cwd()`
unconditionally at `lib.rs:39`, before matching the op, and bails if not inside a
bare container. `init` and explicit-spec `migrate`/`flatten` must run from any
cwd. Restructure `run`: `Init` and explicit-spec conversions operate by spec with
no cwd resolution; only `switch`/`list`/`prune`/`pick` and no-spec conversions
resolve the container from cwd (move that call into the local-op arm).

**Pre-clap dispatch + per-verb inner parsers** (closes findings 3, 5). Extend
`main.rs`'s existing `shell-init` interception (`main.rs:17`) to also intercept
`init`/`migrate`/`flatten` when they are `argv[1]`, each handed to its own clap
struct via `parse_from` (`InitCli`/`MigrateCli`/`FlattenCli`) so
`--clonepath`/`--remote`/`--dry-run`/`[spec]`/`--help` parse and fail closed.
Contract: the verb (or branch) is `argv[1]`; leading global flags before it are
unsupported, exactly as the branch form already is today.

- `worktree init` flags mirror clone's bare-acquisition inputs: `<spec>`,
  `--clonepath` (default `"."`, matching `clone/src/cli.rs:31`), `--remote`
  (default `REMOTE_URLS[0]`, matching `clone/src/cli.rs:28`), `--mirrorpath`
  (optional); ssh_key derived from `common::config`. NOT `--versioning` (a
  flat-only feature that stays on `clone`).
- **Existing-target behavior:** `init` on an existing bare container reconciles
  in place (`reconcile_container`); on an existing flat clone, updates in place
  and prints a `worktree migrate` hint (the behavior `clone --bare` has today via
  `run_bare`), never clobbering.
- **Wrapper:** keep the blanket `-*` passthrough in `worktree/src/shell.rs:35`.
  It correctly routes `--list`/`--prune` (and `-h`/`--help`/`-v`/`--version`)
  through with no `cd`; the verbs are non-`-*` so they land in the capture-and-cd
  branch. Do NOT narrow it to clone's whitelist (that would send `--list`/
  `--prune` to the cd branch). See Resolved Decisions.
- Port `clone/tests/lifecycle_tests.rs` to worktree verbs IN THIS PHASE (closes
  finding 10) so the new network+layout surface ships with e2e coverage; Phase 3
  only deletes the dead clone tests.

**Success criteria:**
- `worktree init org/repo` run from a temp dir OUTSIDE any repo produces `.bare/`
  + a relative `.git` pointer + populated `origin/*` refs + a default-branch
  worktree.
- `worktree migrate org/repo` / `worktree flatten org/repo` run from outside any
  repo (explicit spec); the no-spec forms resolve from cwd.
- `worktree init --help` prints usage (not a git-clone error on `--help`).
- `worktree migrate` then `worktree flatten` yields an identical
  `git for-each-ref` OID set before and after.
- ported e2e lifecycle tests pass; `otto ci` green.

#### Phase 3: Strip clone to flat-only
**Model:** sonnet
- Delete `clone/src/{bare,migrate,flatten,transport}.rs` and their tests.
- Remove `--bare/--migrate/--flatten/--dry-run` (`cli.rs:37-62`), `Layout` enum,
  `run_bare`, `resolve_layout`, `Op::Migrate/Flatten` (`config.rs`).
  `--versioning` stays; `--flat` STAYS as the existing retained no-op alias
  (`cli.rs:44`) so it isn't a second breaking change (see Resolved Decisions).
- Rewrite/relocate clone tests: delete `test_clone_bare_opt_in_layout`, the
  layout/op tests in `config/tests.rs`; keep flat-clone coverage.
- Update `clone/src/shell.rs` wrapper header comment to "flat checkout".
- **Success criteria:** `clone --bare org/repo` exits non-zero with an
  unknown-argument error (same for `--migrate`/`--flatten`); `clone org/repo`
  still flat-clones and `cd`s; `otto ci` green; no `bare`/`migrate`/`flatten`
  symbol remains in the `clone` crate.

#### Phase 4: Shell-init cd-on-init for worktree verbs
**Model:** sonnet
- Ensure `worktree init/migrate/flatten` hit the wrapper's capture-and-`cd`
  path (they are non-`-*` `argv[1]` verbs, so they already do).
- **`--dry-run` prints its preview to stderr and leaves stdout empty**, so the
  wrapper's existing empty-output guard (`shell.rs:41`, `[[ -z "$dest" ]]`)
  short-circuits before any `cd`. This closes the ambiguity in the moved code
  (`flatten.rs:135` currently returns the container path on dry-run, which would
  otherwise `cd` you to the container root from a subdirectory).
- Extend `tests/shell-functions.zsh` with cd-on-init/migrate/flatten cases plus
  a dry-run no-`cd` case, driven by the existing stub harness.
- **Success criteria:** the `shell-test` otto task passes; an emitted
  `worktree()` function `cd`s into the new default worktree after
  `worktree init`; a `--dry-run` invocation prints its preview and leaves you in
  `$PWD` (no `cd`).

#### Phase 5: Config YAML migration + docs true-up
**Model:** sonnet
- Convert `common::config` to YAML-primary (`git-tools.yml`, serde,
  `deny_unknown_fields`, `xdg_config_dir()`), INI fallback to old `clone.cfg`.
- Ship one annotated `git-tools.yml.example` at the repo root (there is no
  existing `clone.cfg` example to supersede).
- Update `CLAUDE.md` (Install/wiring, bare-layout, module map, config), both
  binaries' `after_help`, and flip this doc's Status to Implemented.
- **Success criteria:** `grep -r default-layout` over code+docs is empty; a YAML
  config at the new path is read in preference to the old INI; both `--help`
  outputs match shipped behavior; `whitespace -r` clean.

## Acceptance Criteria

- [ ] `clone --bare org/repo` (and `--migrate`, `--flatten`) exit non-zero with
      an unknown-argument error; `clone org/repo` still flat-clones and `cd`s.
- [ ] `worktree init org/repo` produces a bare container (`.bare/`, relative
      `.git` pointer, populated `origin/*` refs, default-branch worktree) and
      the shell wrapper leaves you inside the default worktree.
- [ ] `worktree migrate` then `worktree flatten` round-trips a repo with an
      identical `git for-each-ref` OID set before and after (no ref lost).
- [ ] A YAML config at `~/.config/git-tools/git-tools.yml` is read by both
      binaries; the old `~/.config/clone/clone.cfg` INI still loads when the
      YAML is absent; `grep -rn default-layout` over `*/src`, `CLAUDE.md`, and
      the `--help` output (excluding `docs/design/**`, which is point-in-time
      history) is empty.
- [ ] `otto ci` green; `whitespace -r` clean; no `bare`/`migrate`/`flatten`/
      transport/`ini` symbol remains in the `clone` crate outside `common`.

## Resolved Decisions

Closed 2026-07-07 during scoping (Scott + design author):

- **Transport home:** extract to `common::transport`, both binaries consume it;
  no cross-binary dependency. (Scott)
- **`default-layout`:** removed entirely; bare is purely explicit via
  `worktree init`/`migrate`. (Scott)
- **Config file:** rename to `~/.config/git-tools/git-tools.yml` (YAML), convert
  from INI, keep reading the old `clone.cfg` INI path as a fallback. (Scott)
- **Module ownership:** `bare`/`migrate`/`flatten` land in the `worktree` crate,
  not `common` (worktree owns the lifecycle). Phasing cost: Phase 2 copies,
  Phase 3 strips clone, so each phase stays green. (author, matches vision)
- **Verb surface:** reserved-word positionals (`worktree init <spec>`) with
  pre-clap dispatch, not flags. Forced by the wrapper's `-*` pass-through (flags
  would silently skip `cd`) and clap positional ambiguity. (author, forced)
- **`clone --bare` back-compat:** hard unknown-argument error, no shim. (author,
  per handoff "disappears entirely")

Closed 2026-07-07 from the review-panel consensus loop (Architect/Gemini +
Staff Engineer/Codex, Design Review). All findings verified against code; every
one folded into the phases above except one disposition corrected:

- **Phase 2 is adapt-not-copy** (findings 1, 2, 5). `clone/src/bare.rs` is
  coupled to a clone-shaped `Config` and `worktree/src/bare.rs` already exists, so
  the modules merge behind a scalar `AcquireArgs`; dispatch splits so `init` +
  explicit-spec conversions run from any cwd; verbs use per-verb inner clap
  parsers via pre-clap interception. Folded into Phase 2.
- **`worktree init --clonepath` default is `"."`** (finding 4), matching
  `clone/src/cli.rs:31` (the doc previously mis-stated `~/repos`). Sibling
  symmetry with `clone`; the persona invariant holds when run from `~/repos`,
  same precondition as `clone` today. (author)
- **`worktree init` mirrors `--remote`** (finding 11), default `REMOTE_URLS[0]`,
  matching `clone/src/cli.rs:28`. Sibling symmetry. (author)
- **`--dry-run` prints to stderr with empty stdout** (finding 9) so the wrapper's
  empty-output guard prevents any `cd`. Folded into Phase 4.
- **Config load is fail-closed** (finding 6): first existing file wins; a parse
  failure is loud and does NOT fall through; absent file falls through; key/org
  lookups return `None` not an error. Folded into Data Model.
- **`clone --flat` stays a no-op alias** (finding 8). Only `--bare`/`--migrate`/
  `--flatten` were scoped for removal; dropping `--flat` too would be a second,
  unrequested breaking change. (author)
- **DIVERGENCE from reviewer disposition (finding 3):** the reviewers proposed
  narrowing the `worktree()` wrapper's `-*` case to a whitelist "matching clone's".
  REJECTED with rationale: clone's whitelist is `-h|--help|-v|--version|shell-init`,
  but `worktree` also needs `--list`/`--prune` to pass through with no `cd`; a
  clone-style whitelist would route them to the capture-and-cd branch and `cd`
  into table output. The blanket `-*` passthrough is correct for `worktree`. The
  finding's real concern (leading flags before a verb) is closed by the
  flag-ordering contract (verb/branch is `argv[1]`) + `argv[1]` interception, not
  a whitelist. (author; surfaced to Scott)

## Alternatives Considered

### Alternative 1: Merge clone into worktree as a subcommand
- **Description:** Make `worktree` the top-level tool; `worktree clone ...`.
- **Cons:** Breaks the fleet's one-binary-per-verb convention; inverts usage
  frequency (most users never touch `worktree`).
- **Why not chosen:** Conformance to the org/fleet pattern beats consolidation.

### Alternative 2: worktree depends on the clone lib crate for transport
- **Description:** `worktree` pulls in `clone` as a lib dep, calls its transport.
- **Cons:** Couples the minority tool to the majority tool; wrong dependency
  direction; drags the whole `clone` crate into `worktree`.
- **Why not chosen:** Scott chose `common::` extraction; clean decompose.

### Alternative 3: Leave migrate/flatten on clone (status quo)
- **Why not chosen:** This is the cognitive-dissonance smell the doc exists to
  remove; transport and layout stay coupled.

### Alternative 4: Flag-form verbs (`worktree --init`)
- **Cons:** The `worktree()` wrapper passes `-*` args through with no `cd`, so
  `--init` would silently regress cd-on-init; clap would also need custom
  handling to avoid arg ambiguity.
- **Why not chosen:** Positionals + pre-clap dispatch are the correct fit.

### Alternative 5: Keep `clone.cfg` name and INI format
- **Why not chosen:** Both binaries read it now, so the name lies; INI violates
  the YAML house rule.

## Technical Considerations

### Dependencies
- `common` gains `ini` (for the fallback INI read) and reuses the workspace's
  existing `serde_yaml = "0.9"` (already a `common` dep, used by `language.rs`)
  for the YAML read.
- `clone` drops its direct `ini` dependency (the reader moved to `common`); after
  Phase 1 nothing in `clone` imports `ini` directly.
- No new external services. No cross-repo code dependency introduced.

### Performance
- No hot paths. Transport is a one-shot subprocess `git clone`; config read is a
  single file parse. No change in characteristics.

### Security
- **Persona invariant preserved (with the same precondition as clone today).**
  `worktree init`'s `--clonepath` defaults to `"."` (matching `clone/src/cli.rs:31`,
  NOT `~/repos`), so the container lands at `<cwd>/<org>/<repo>`. The invariant
  (`~/.gitconfig` `includeIf "gitdir:~/repos/tatari-tv/"` firing so commits carry
  the work identity) holds when `init` is run from under `~/repos`, exactly as
  `clone` relies on today. This is not an automatic guarantee of the tool; it is a
  property of where you run it. The locking test
  (`test_persona_invariant_under_org_prefix`, currently `clone/src/bare/tests.rs:202`)
  moves with `bare.rs` into `worktree` and asserts safety at an explicit
  `~/repos`-prefixed clonepath.
- **Fail-closed config:** `deny_unknown_fields` on the YAML struct; an
  unparseable config is a loud error, never a silent empty result.
- **rkvr-only deletes:** `migrate`/`flatten` keep routing destructive ops
  through `common::rkvr`; no raw `rm`.
- SSH-key selection logic is moved verbatim, not rewritten; same per-org key
  resolution.

### Testing Strategy
- Relocated unit tests move with their modules (`bare`/`migrate`/`flatten`
  tests -> worktree), unchanged logic.
- clone's e2e lifecycle tests (`clone/tests/lifecycle_tests.rs`, currently
  exercising `--migrate`/`--flatten`/`--bare`) are rewritten to drive
  `worktree init/migrate/flatten`.
- Round-trip ref-preservation asserted via `git for-each-ref` OID-set equality.
- Shell wrapper behavior locked by `tests/shell-functions.zsh` (the `shell-test`
  otto task): prints-path, does-the-`cd`, and the failed-op guard for each new
  verb.
- Tests must bite: break a relocated migrate/flatten assertion to prove it fails
  before landing.

### Rollout Plan
- Single-repo change, single release, all crates bump together (flat `v*` tag).
- No dotfiles change required: the `.zshrc` `eval` lines (`clone shell-init`,
  `worktree shell-init`) and `manifest.yml` cargo list are unchanged (same
  binaries, same crate names).
- Operators with `~/.config/clone/clone.cfg` need no manual migration: the INI
  fallback read keeps working until they move to `git-tools.yml`.
- Muscle-memory break: anyone running `clone --bare` gets a hard error and must
  switch to `worktree init`. Called out in release notes.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Phase 2 duplication left uncleaned | Low | Med | Phase 3 strip is a hard gate; acceptance criterion asserts no bare/migrate/flatten symbol in clone |
| cd-on-init silently regresses via wrapper | Med | High | Positionals + pre-clap dispatch; `shell-test` cases assert the `cd` for each verb |
| Persona invariant lost when moving worktrees | Low | High | Locking test moves with `bare.rs`; org-prefix assertion unchanged |
| Old clone.cfg users broken by YAML switch | Low | Med | INI fallback read of the old path; no manual migration required |
| clap swallows `init`/`migrate`/`flatten` as branch names | Med | Med | Pre-clap interception in `main.rs`, mirroring `shell-init` |

## Open Questions

- [ ] None. All decisions resolved (see Resolved Decisions).

## Addendum: Parked / rejected, with reasoning

- **Remove the INI `clone.cfg` fallback read.** Parked. Revisit once the fleet
  has migrated to `git-tools.yml`; removing it now would break existing
  machines. Revisit condition: no machine reads the old path (survey).
- **`clone --bare` deprecation shim.** Rejected. The handoff calls for it to
  disappear entirely; a hard error is the clean break. A shim reintroduces the
  coupling the split removes.
- **A branch literally named `init`/`migrate`/`flatten`.** Low-risk collision:
  as the first positional, these words bind to the acquisition verb, so
  `worktree init` cannot switch to a branch named `init`. No escape hatch is
  planned (the three names are rare branch names). Accepted and documented; if it
  ever bites, fall back to `git worktree add` for that one branch.

## References

- Handoff: `/tmp/claude-1000/handoff-clone-worktree-architecture.md`
- `docs/design/2026-06-21-clone-bare-worktree.md` (bare layout)
- `docs/design/2026-06-28-worktree-tool.md` (worktree tool)
- `docs/design/2026-06-28-consolidate-worktree-primitives.md` (shared primitives)
- `docs/design/2026-07-03-clone-flat-default.md` (flat default)
- Repo `CLAUDE.md` (Install & Wiring, wrapper contract, bare-worktree layout)
