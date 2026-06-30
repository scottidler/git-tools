# Design Document: `shell-init` subcommand for `clone` and `worktree`

**Author:** Scott A. Idler
**Date:** 2026-06-29
**Status:** Implemented
**Review Passes Completed:** 5/5 + review-panel (Architect + Staff Engineer)

## Summary

Convert the git-tools `clone` and `worktree` binaries to emit their own `cd`-wrapper
shell functions via a `<bin> shell-init <shell>` subcommand, installed with one line
in `.zshrc` (`eval "$(clone shell-init zsh)"`). This matches the house pattern already
used by `qai` and `aka`, and retires the static, checked-in `shell-functions.sh` file
plus its `manifest`-managed symlink, collapsing the function definition into a single
binary-owned source of truth so the live shell copy can never again drift from the repo.

## Problem Statement

### Background

`clone` and `worktree` are binaries that print a destination directory to **stdout**;
a shell **function** of the same name captures that stdout and `cd`s the *parent* shell
into it (a child process cannot `cd` its parent). Today those two functions ship as a
single static file, `git-tools/shell-functions.sh`, which `manifest` symlinks to
`~/.shell-functions.d/git-tools.sh` and `~/.shell-functions` auto-sources at shell
startup.

The newer house pattern (see `qai shell-init zsh`, `aka shell-init zsh`, both live in
`~/.zshrc:142,146`) is for the **binary to emit its own function**, installed with one
`eval "$(... shell-init zsh)"` line. This moves `clone`/`worktree` onto that pattern.

This design supersedes the pre-existing punch lists at `/tmp/clone-shell-init-handoff.md`
and `~/repos/scottidler/clone/docs/shell-init-conversion.md`. Both were written against
the **retired Python 2** repo `~/repos/scottidler/clone`; the live tools are the **Rust
git-tools** binaries, so their Py3-port "blocker", `func/clone.sh`, and `install.sh`
items do not apply. Their *intent* (emit the robust function, drop `$CLONE`, protect the
stdout contract, kill the drift) carries over and is captured here.

### Problem

1. **Drift.** The function lives in two places that can diverge: `shell-functions.sh` in
   the repo and whatever copy is actually loaded in a given shell. There is no mechanism
   forcing them to agree.
2. **Multi-step install.** Bringing the functions to a machine requires the
   `manifest` symlink dance (`shell-functions.sh → ~/.shell-functions.d/git-tools.sh`),
   not a single documented line, and diverges from how `qai`/`aka` install.
3. **Inconsistency with the house pattern.** Every other recent tool emits its own
   shell integration; these two are the odd ones out.

### Goals

- `clone shell-init <shell>` and `worktree shell-init <shell>` print the respective
  wrapper function to stdout, installable via `eval "$(... shell-init zsh)"`.
- The emitted functions bake in the robust behavior the live copies already have
  (help/version passthrough, stdout capture, destination validation before `cd`).
- One binary-owned source of truth per function; no second static copy.
- `zsh` supported first; the emitter is structured so `bash`/`fish` can be added later
  without a rewrite; an unknown shell name errors clearly and non-zero.
- The existing bare-positional interfaces are preserved unchanged:
  `clone org/repo [revision]` and `worktree <some-name>` (switch-or-create) /
  `worktree` (fzf picker) / `worktree -L|--list|--prune`.
- Retire `shell-functions.sh` and its `manifest` link; document the one-line install.

### Non-Goals

- Shipping `bash`/`fish` bodies now (structure for them; implement `zsh` only).
- Touching the retired Python `~/repos/scottidler/clone` repo.
- Changing the binary-prints-path / function-does-the-`cd` contract (CLAUDE.md "wrapper
  contract") — this design preserves it exactly.
- Reintroducing any `chpwd`/`cd`-interception navigation magic (deliberately removed
  earlier; see CLAUDE.md "No `cd` navigation magic").
- A general plugin/subcommand framework — `shell-init` is the only reserved word added.

## Proposed Solution

### Overview

Add a `shell-init` path to each binary that is **dispatched before `clap` parses**, then
a per-crate `shell` module that returns the function text for a requested shell, backed by
a small shared `common::shell` for the supported-shell list and the uniform rejection
error. Then coordinate the retirement of the static file across `.zshrc`, the dotfiles
`manifest.yml`, and `CLAUDE.md`.

### Architecture

```
common/src/shell.rs        # SUPPORTED shells + unsupported(bin, shell) -> Report (one home)
  common/src/shell/tests.rs

clone/src/shell.rs         # init_script(shell) -> Result<String>; const ZSH body (clone())
  clone/src/shell/tests.rs
clone/src/main.rs          # pre-dispatch: argv[1]=="shell-init" -> emit, before Cli::parse()

worktree/src/shell.rs      # init_script(shell) -> Result<String>; const ZSH body (worktree())
  worktree/src/shell/tests.rs
worktree/src/main.rs       # same pre-dispatch
```

#### Why pre-dispatch instead of a `clap` subcommand

`qai` makes `shell-init` a real `clap` subcommand because *every* `qai` invocation is a
subcommand — it has no leading optional positional. `clone` and `worktree` do: `clone`'s
first positional is `[REPOSPEC]`, `worktree`'s is `[BRANCH]`. Today's binaries have no
subcommand at all, so the first token is parsed straight into the positional — confirmed:

```
$ clone shell-init zsh
Error: Failed to parse repository specification: shell-init   # "shell-init" became REPOSPEC
```

This proves only that *today's* (no-subcommand) binary eats the token, not that a
subcommand could never coexist. A real subcommand *can* be made to live alongside a
leading optional positional via `args_conflicts_with_subcommands` /
`subcommand_precedence_over_arg` / `external_subcommand` — but each either makes the
positional and the subcommand mutually exclusive, or turns an unknown positional into an
"unrecognized subcommand" error, i.e. it puts the primary `clone org/repo` /
`worktree <branch>` interface at risk. Rather than restructure that interface, we reserve
exactly one token *before* clap: `main.rs` inspects `std::env::args()` and, **only** when
the exact first argument is `shell-init`, emits and returns before `Cli::parse()`. clap
never sees the token; the positional path is byte-for-byte unchanged. The reserved
surface is exactly one word, and extra args after the shell name are ignored (`clone
shell-init zsh extra` emits the zsh script).

### Data Model

No persisted state. The emitted scripts are `&'static str` constants. `common::shell`
holds only the *shared formatting* of the rejection — the supported set is passed in by
the caller so it reflects each crate's actual truth (a global `SUPPORTED = ["zsh"]` would
lie the moment one tool gains `bash` before the other):

```rust
/// Uniform "unsupported shell" error. `supported` is the calling crate's own
/// list, so the message never claims a shell a given tool doesn't actually emit.
pub fn unsupported(bin: &str, supported: &[&str], shell: &str) -> eyre::Report;
```

Each crate owns its supported list next to its bodies (e.g. `const SUPPORTED: &[&str] =
&["zsh"];` in `clone/src/shell.rs`), and `init_script` matches on it.

### API Design

Per-crate emitter (identical signature in `clone` and `worktree`):

```rust
const SUPPORTED: &[&str] = &["zsh"];   // per-crate truth, next to the bodies

/// The shell-init script for `shell`, or an error naming the supported shells.
pub fn init_script(shell: &str) -> eyre::Result<String> {
    match shell {
        "zsh"  => Ok(ZSH.to_string()),
        other  => Err(common::shell::unsupported("clone" /* or "worktree" */, SUPPORTED, other)),
    }
}
```

`main.rs` pre-dispatch (identical shape in both, default shell `zsh`):

```rust
let mut raw = std::env::args().skip(1);
if raw.next().as_deref() == Some("shell-init") {
    let target = raw.next().unwrap_or_else(|| "zsh".to_string());
    print!("{}", shell::init_script(&target)?);   // stdout = the script, nothing else
    return Ok(());
}
let cli = Cli::parse();   // unchanged positional/flag path below
```

#### Emitted `clone()` (zsh)

```zsh
# clone - smart git clone (bare-worktree layout) [shell-init <GIT_DESCRIBE>]
# Install: add to your .zshrc -> if hash clone 2>/dev/null; then eval "$(command clone shell-init zsh)"; fi
clone() {
    if [[ "$1" == (-h|--help|-v|--version|shell-init) ]]; then
        command clone "$@"
    else
        local dest
        dest=$(command clone "$@") || return $?
        if [[ -z "$dest" || ! -d "$dest" ]]; then
            print -u2 -- "clone: no valid destination returned; staying in $PWD"
            return 1
        fi
        cd "$dest"
    fi
}
```

#### Emitted `worktree()` (zsh)

```zsh
# worktree - switch/create git worktrees in a bare container [shell-init <GIT_DESCRIBE>]
# Install: add to your .zshrc -> if hash worktree 2>/dev/null; then eval "$(command worktree shell-init zsh)"; fi
worktree() {
    case "$1" in
        -*|shell-init)
            command worktree "$@"
            ;;
        *)
            local dest
            dest=$(command worktree "$@") || return $?
            if [[ -z "$dest" || ! -d "$dest" ]]; then
                print -u2 -- "worktree: no valid destination returned; staying in $PWD"
                return 1
            fi
            cd "$dest"
            ;;
    esac
}
```

Several behaviors are deliberate:
- **`command clone` / `command worktree`** inside the body (not `$CLONE`/`$WORKTREE`).
  The emitted form resolves the on-PATH binary directly and `command` bypasses the
  same-named function, so there is no env var to go unset and no self-shadowing. *Behavior
  change vs the old body:* `shell-functions.sh` snapshotted the path with `=clone` at
  source time; `command clone` resolves at call time, so a later PATH shim is honored. For
  these tools that is the desired behavior.
- **`shell-init` is in the passthrough of BOTH functions.** In `worktree()` it joins the
  `-*` case; in `clone()` it joins the `(-h|--help|-v|--version)` test. Without it, an
  interactive `<bin> shell-init zsh` *after* the function is loaded falls into the capture
  branch, swallows the emitted script as `dest`, and fails the `! -d` guard. (An earlier
  draft argued `clone()` did not need this because its install-time `eval` calls the
  binary — that only holds *before* the function exists; once loaded, the function
  shadows. Both functions need the passthrough.)
- **Install line uses `command <bin>` and a `hash` guard:**
  `if hash clone 2>/dev/null; then eval "$(command clone shell-init zsh)"; fi`. The
  `command` is load-bearing during cutover: if an old `clone()` function is still defined
  (e.g. a machine that pulled the new `.zshrc` before `manifest` removed the old symlink),
  a bare `eval "$(clone shell-init zsh)"` would call that old function, capture the script,
  and define nothing. `command clone` always runs the binary. The `hash` guard matches the
  `qai`/`aka` lines (`~/.zshrc:141,145`) and degrades gracefully (no wrapper, no startup
  error) when the binary isn't installed. This is the fix for the cutover-ordering hazard
  in the Transition section.
- **Version marker.** The header comment carries the binary's `GIT_DESCRIBE`, so a stale
  function in a long-running shell (the `eval` snapshots at startup) is diagnosable by
  comparing the comment to `clone --version`.

### Implementation Plan

#### Phase 1: Shared `common::shell` scaffolding
**Model:** sonnet
- Add `common/src/shell.rs`: `unsupported(bin, supported, shell) -> Report` (the supported
  set is a parameter, not a shared global).
- Register `pub mod shell;` in `common/src/lib.rs`.
- `common/src/shell/tests.rs`: assert `unsupported` names the command, echoes the bad
  shell, and lists the passed-in supported set.

#### Phase 2: `clone` emitter + pre-dispatch
**Model:** sonnet
- Add `clone/src/shell.rs` (`init_script` + `ZSH` const) and `pub mod shell;` in
  `clone/src/lib.rs`.
- Add the pre-dispatch block to `clone/src/main.rs` before `Cli::parse()`.
- `clone/src/shell/tests.rs`: script defines `clone()`, uses `command clone`, carries the
  install line, drops `$CLONE`; emitted zsh passes `zsh -n` (skipped if zsh absent);
  unknown shell rejected.

#### Phase 3: `worktree` emitter + pre-dispatch
**Model:** sonnet
- Mirror Phase 2 in `worktree/src/{shell.rs,main.rs,lib.rs}`, with the `worktree()` body
  and `shell-init` added to the passthrough case.
- `worktree/src/shell/tests.rs`: as Phase 2, plus assert `-*|shell-init)` passthrough.

#### Phase 4: Retire the static wiring (clean break, ordered)
**Model:** opus

`shell-functions.sh` is referenced by four systems beyond the file itself; a clean break
(no compat release) handles all of them in this changeset. Deleting the file without these
would turn `otto ci` red, ship a broken tarball, and break `manifest` on every machine.

CI / test (`.otto.yml`):
- The `shell-test` task (`.otto.yml:35-39`) runs `zsh tests/shell-functions.zsh`, wired as
  a CI dependency (`.otto.yml:165` `before: [lint, check, test, shell-test]`). **Retarget**
  the fixture `tests/shell-functions.zsh` to validate the *emitted* functions instead of
  the static file: `eval "$(target/<profile>/clone shell-init zsh)"`, then assert the
  binary-prints-path / function-does-`cd` contract and the failed-clone guard (and the same
  for `worktree`). Keep the task and the CI dep — the contract test is worth keeping; only
  its subject changes.

Release tarball (`.github/workflows/binary-release.yml`):
- Remove `cp shell-functions.sh artifacts/` (`binary-release.yml:61-62`). The tarball no
  longer ships the file.

Installer (`~/repos/scottidler/dotfiles/manifest.yml`):
- Remove the `link:` entry `shell-functions.sh: ~/.shell-functions.d/git-tools.sh`
  (`manifest.yml:361`).
- Remove `mv ~/bin/shell-functions.sh ~/.shell-functions.d/git-tools.sh` from the
  `git-tools` post-install block (`manifest.yml:617`); the block keeps extracting the
  binaries to `~/bin/`.
- Add the install to the dotfiles-tracked `.zshrc`, near `qai`/`aka` (`~/.zshrc:141-147`):
  `if hash clone 2>/dev/null; then eval "$(command clone shell-init zsh)"; fi` and the
  `worktree` equivalent.
- Run `manifest` so the symlink removal + `.zshrc` update apply reproducibly (never
  hand-edit the live symlink).

Repo:
- `rkvr rmrf shell-functions.sh`.
- Update `git-tools/CLAUDE.md`: replace the `shell-functions.sh` "Shell function" section
  and the "Live wiring" / manifest `link:` notes with the `shell-init` install, preserving
  the wrapper-contract documentation (binary-prints-path / function-does-`cd`).

Verification (note: `whence -w clone` reports "function" for BOTH the old and new wiring,
so it does NOT prove the cutover — verify the *source* of the function):
- Fresh login shell after `manifest`: `functions clone | grep 'command clone'` (or the
  `shell-init <version>` header comment) confirms the binary-emitted body is loaded, not
  the old static one.
- `clone org/repo` and `worktree <name>` still create-and-`cd`; `otto ci` green; a test
  release tarball no longer contains `shell-functions.sh`.

## Alternatives Considered

### Alternative 1: Real `clap` subcommand (like `qai`/`aka`)
- **Description:** Add `#[command(subcommand)] command: Option<Commands>` with a
  `ShellInit { shell }` variant.
- **Pros:** Identical to `qai`/`aka`; free `--help` integration; appears in `--help`.
- **Cons:** Coexisting with a leading optional positional is fragile — today's binaries
  route the first token into `[REPOSPEC]`/`[BRANCH]` (confirmed: `clone shell-init zsh` →
  `repospec="shell-init"`). Making a subcommand coexist needs
  `args_conflicts_with_subcommands` / `subcommand_precedence_over_arg` /
  `external_subcommand`, each of which either makes args and the subcommand mutually
  exclusive or turns an unknown positional into an "unrecognized subcommand" error — i.e.
  it puts `clone org/repo` / `worktree <branch>` at risk.
- **Why not chosen:** Preserving the bare-positional interface is a hard goal; the
  pre-dispatch reserves exactly one token and leaves `clap` untouched. (We did *not* prove
  a subcommand is impossible — only that it threatens the primary interface for no benefit
  the one-token reservation doesn't already give.)

### Alternative 2: Keep the static `shell-functions.sh` + symlink
- **Description:** Do nothing; keep the checked-in file and `manifest` link.
- **Pros:** Zero work.
- **Cons:** Leaves the drift and multi-step-install problems unsolved; stays off the
  house pattern.
- **Why not chosen:** Those are the problems this design exists to fix.

### Alternative 3: Put the whole emitter in `common` (pass bodies in)
- **Description:** A single `common::shell::select(bin, shell, zsh_body, ...)` that owns
  the match.
- **Pros:** Slightly more centralized.
- **Cons:** The bodies differ per tool and the signature grows awkwardly as shells are
  added (`select(bin, shell, zsh, bash, fish)`); bodies are most readable next to the
  tool they belong to.
- **Why not chosen:** Keep bodies local; share only the supported-set + error (the parts
  that must stay identical). The shared surface is intentionally minimal.

## Technical Considerations

### Dependencies
- Internal: both crates already depend on `common` (`common = { workspace = true }`); no
  new crates.
- External: none. The `zsh -n` test shells out to `zsh` if present and skips otherwise.
- Cross-repo: Phase 4 edits `~/repos/scottidler/dotfiles/manifest.yml` and `~/.zshrc`,
  which live outside git-tools.

### Performance
- Irrelevant: emission is a constant-string print; pre-dispatch is one `argv` comparison
  on every invocation (negligible, and only when the first arg is literally `shell-init`).

### Security
- No new attack surface. `command clone`/`command worktree` resolve the on-PATH binary —
  consistent with the existing wrapper, which is what the user already runs.
- Persona invariant (worktrees stay under `~/repos/<org>/`) is unaffected; this changes
  only how the wrapper is *delivered*, not where it operates.

### Testing Strategy
- Unit tests per crate: emitted script defines the function, uses `command <bin>`, carries
  the install line, omits `$CLONE`/`$WORKTREE`; unknown shell rejected with a message that
  names the command and the bad shell.
- `zsh -n` parse check on the emitted script, gated on `zsh` being installed (skip-print
  if absent so CI without zsh still passes).
- `otto ci` green for the whole workspace.
- Manual: `eval "$(target/debug/clone shell-init zsh)"; whence -w clone` defines a
  function; `clone org/repo` and `worktree <name>` still create-and-`cd`.

### Rollout Plan
- Phases 1-3 are pure additive code in the `shell-init` worktree; the static
  `shell-functions.sh` and its `shell-test` keep working unchanged, so CI stays green and
  no shell loses its functions.
- Phase 4 is the cutover: it retargets the contract test, drops the release `cp` and the
  `manifest` link/installer-`mv`, adds the guarded `eval` lines to `.zshrc`, and deletes
  the file. Released via `/shipit` (`bump` patch, synchronized across crates, then the
  `v*` tag triggers the release tarball, then `otto install`).

### Transition and Rollback

The steady state (a fresh shell, file deleted, `eval` lines present) is easy; the
transition has three states that must all work:

1. **Fresh login shell, post-cutover.** `manifest` has removed the
   `~/.shell-functions.d/git-tools.sh` symlink, so `~/.shell-functions` (sourced at
   `~/.zshrc:100`) no longer defines `clone`/`worktree`. The guarded `eval` lines later in
   `.zshrc` then define them from the binary. Works.
2. **Machine that pulled the new `.zshrc` before `manifest` removed the old symlink.** Here
   `~/.shell-functions` still defines the old `clone()`/`worktree()` *before* the `eval`
   lines run. This is the hazard the panel caught: a bare `eval "$(clone shell-init zsh)"`
   would call the old *function*, capture the script, and define nothing. The `command`
   in the install line (`eval "$(command clone shell-init zsh)"`) defeats this — it runs
   the binary regardless of the shadowing function, and the resulting `eval` *redefines*
   `clone()` with the new body. So even out-of-order, the new body wins.
3. **Already-running shell at cutover time.** It keeps whatever functions it loaded until
   the user starts a new shell. No corruption; the version-marker comment makes a stale
   function diagnosable. Document "open a new shell" as the activation step.

**Rollback:** revert the `.zshrc` `eval` lines and restore the `manifest.yml`
link/installer entries, then run `manifest` (re-creates the symlink, re-installs the file
from the prior release). The `hash <bin>` guard also means that if the binary is ever
missing, the worst case is "no `clone`/`worktree` wrapper" (a plain command-not-found on
use), never a broken shell startup.

## Risks and Mitigations

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Old `clone()`/`worktree()` already loaded when the `eval` line runs (cutover ordering) → eval calls the function, defines nothing | Med | High | Install line uses `eval "$(command clone shell-init zsh)"`; `command` runs the binary regardless of the shadowing function, and the eval *redefines* the function with the new body. See Transition. |
| Deleting `shell-functions.sh` breaks CI / release tarball / `manifest` | High (if missed) | High | Phase 4 enumerates and updates all four: `.otto.yml` shell-test (retargeted, not removed), `binary-release.yml:61-62` cp removed, `manifest.yml:361` link + `:617` installer-`mv` removed. |
| Branch literally named `shell-init` not creatable via `worktree shell-init` | Low | Low | **Accepted by decision.** Reserved unconditionally; documented. Not a realistic branch name. |
| Re-emitting interactively (`clone`/`worktree shell-init`) gets captured and `cd`-attempted | Low | Low | `shell-init` is in the passthrough of **both** emitted functions, so it prints instead of being captured. |
| `manifest` not run after editing `manifest.yml`/`.zshrc`, leaving a stale symlink/install | Low | Med | Phase 4 runs `manifest`; never hand-edit the symlink. |
| CLAUDE.md left describing the deleted `shell-functions.sh` | Med | Low | Phase 4 updates CLAUDE.md as part of the cutover. |

## Resolved Questions (post review-panel, 2026-06-29)
- **`common::shell` shared or inlined?** *Resolved:* keep it shared, but
  `unsupported(bin, supported, shell)` takes the supported list as a **parameter** (no
  shared global), so per-crate truth is preserved while the error format stays uniform.
- **Phase 4 in this change set or a follow-up?** *Resolved (user decision):* **clean break
  in one change** — Phase 4 retargets/removes every `shell-functions.sh` touchpoint and
  deletes the file in this changeset; no compat release.
- **`worktree shell-init` vs a branch named `shell-init`?** *Resolved (user decision):*
  **accept + document** — reserve `shell-init` unconditionally; a branch by that name isn't
  creatable via the wrapper. No escape hatch.

## Open Questions
- [ ] `--help` discoverability: with pre-dispatch, `shell-init` won't appear in
  `clone --help`/`worktree --help`. Add a one-line mention to `about`/`after_help`
  (recommended), or leave it undocumented in `--help`?

## References
- Handoff: `/tmp/clone-shell-init-handoff.md` (written against the retired Python repo)
- Spec: `~/repos/scottidler/clone/docs/shell-init-conversion.md` (same caveat)
- House pattern: `~/repos/scottidler/qai/src/{cli.rs,shell.rs,main.rs}`;
  `~/.zshrc:142,146`
- Live functions today: `git-tools/shell-functions.sh`
- Wrapper contract + wiring: `git-tools/CLAUDE.md` (Install & Wiring, Bare-Worktree Layout)
