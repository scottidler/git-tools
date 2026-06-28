# CLI Shakedown Report: git-tools v0.2.13

Date: 2026-06-28. Host: Linux x86_64. Focus: the freshly shipped **`worktree`** tool
(bare-container worktree switch/create/list/prune) and its `worktree()` shell-function
wrapper. Binaries exercised through the real `worktree()` zsh function (sourcing the
shipped `shell-functions.sh`) so the cd-wrapper and arg-forwarding are tested, not just
the binary. All sandboxes are throwaway bare containers built in `/tmp` — never the real
`~/repos`.

## Summary

| Metric | Count |
|--------|-------|
| Tool under test | `worktree` (single command, flag-driven) |
| Flows tested | switch, create, list, picker, prune, all flag conflicts |
| Passed | all core flows |
| Failed | 0 crashes; **3 real bugs found and fixed** (below) |
| Skipped (mutating) | none — exercised in `/tmp` sandboxes |

Verdict: **`worktree`'s behavior is correct end-to-end** — slug derivation, slash→dash
dir naming, idempotent re-switch, slug-collision guard, the no-arg picker's non-interactive
failure, and the full prune-protection matrix all verified. Shakedown surfaced three real
defects — a shell word-split bug, a list-alignment bug, and a release-packaging bug that
**omitted the `worktree` binary from the v0.2.13 tarball** — all fixed for the next patch.

## worktree — behavior (primary focus)

Sandbox: a local fixture origin (`main` + a merged `feature/auth` + an `unmerged` branch)
cloned into a bare container the way `clone` builds one (`.bare/`, `.git`→`gitdir: ./.bare`,
refspec fix + fetch, default-branch worktree, `origin/HEAD`→`main`).

| # | Scenario | Result |
|---|----------|--------|
| A | `--list` / `-L` | ✅ table to stdout, no cd; bare container row skipped |
| B | `worktree feature/auth` (slashed branch) | ✅ creates worktree, cd into `…/feature-auth` (dir slugified, branch keeps real name) |
| C | Idempotent re-switch | ✅ cd back into the existing worktree, no error |
| D | `worktree "New Feature"` (spaces) | ✅ **after fix** — one arg → new branch slugified to `new-feature`, cd into `…/new-feature` |
| E | `worktree unmerged` | ✅ switches to an unmerged-branch worktree |
| F | `--prune --yes` | ✅ removes only merged+clean (`feature-auth`); keeps current/default `main`, the unmerged worktree, and a local-only unmerged worktree |
| — | prune keeps branch refs | ✅ `feature/auth` ref survives even though its worktree was removed (prune removes worktrees, never branch refs) |

Earlier direct-binary smoke tests (prior session) also confirmed: empty-slug error,
`--list`+branch error, `--list`+`--prune` mutual-exclusion, no-arg picker failing explicitly
(not hanging) when non-interactive, `--prune` without `--yes` bailing non-interactively,
prune protecting current/dirty/locked/detached, and a clean "not inside a git repository"
error outside a bare container.

## Failures & Bugs (all fixed)

1. **`worktree()` / `clone()` shell wrappers word-split a spaced arg** (correctness).
   Both functions ran `eval $WORKTREE "$@"` / `eval $CLONE "$@"`. `eval` re-parses its
   already-split arguments, so `worktree "New Feature"` reached the binary as **two** args
   (`error: unexpected argument 'Feature' found`). The `eval` was redundant: `$WORKTREE`/
   `$CLONE` already hold the fully-expanded binary path (via `print -r -- =worktree` at
   definition time), so `"$WORKTREE" "$@"` works and preserves quoting.
   - **Fix:** dropped `eval` in both functions (`shell-functions.sh`).
   - **Regression guard:** `tests/shell-functions.zsh` now records the binary's received
     argv and asserts `worktree "New Feature"` arrives as exactly **one** arg, verbatim.
     Confirmed the guard fails against the old `eval` form (argc=2) and passes against the
     fix (argc=1). The test runs in `otto ci`.

2. **`worktree --list` column misaligns on long branch names** (formatting).
   The branch column was padded with a hardcoded `{:<28}`; any branch longer than 28 chars
   overflowed and the path column lost alignment.
   - **Fix:** `print_entries` (`worktree/src/main.rs`) now computes the column width from the
     widest branch name in the entries (char-count, not byte-count), so paths stay aligned
     regardless of branch length.

3. **`worktree` binary missing from the v0.2.13 release tarball** (release packaging).
   `.github/workflows/binary-release.yml` built binaries from a **hardcoded** project list
   (`clone reposlug ls-git-repos …`) that never included `worktree`. The v0.2.13 tarball
   shipped every other tool plus `shell-functions.sh` but **not the tool the release was cut
   for**.
   - **Fix:** the build step now builds `--workspace --bins` and enumerates binary targets
     dynamically via `cargo metadata … | jq 'select(.kind[] == "bin")'` (skips lib-only
     `common`), mirroring how `otto install` loops over the workspace. Any future tool is
     released automatically — no list to maintain. Verified the enumeration lists exactly the
     8 binaries including `worktree`.

## Release Validation (v0.2.13)

- **Tag `v0.2.13`:** annotated, on `origin/main` (verified prior session).
- **Workflow `binary-release`:** completed; release published by `github-actions[bot]`.
- **Asset:** `git-tools-v0.2.13-linux.tar.gz` (11 MB, Linux-only single bundle — the repo's
  release design, not a regression).
- **Binary test:** downloaded + extracted the tarball; `./clone --version` → `clone v0.2.13`,
  `./reposlug --version` → `reposlug v0.2.13`. **But `worktree` was absent from the archive**
  (Bug 3). The tarball validation is what surfaced the packaging bug.

## Observations

- The three fixes (Bugs 1–3) are uncommitted on `main` and want a follow-up patch release;
  Bug 3's workflow fix only takes effect on the *next* tag push, and Bug 2's alignment fix
  needs a reinstalled binary to be visible locally.
- `worktree` (the binary) is shadowed by the `worktree()` shell function in interactive
  shells; binary-level testing uses the full path (`~/.cargo/bin/worktree`) or `=worktree`,
  same caveat as `clone`.
- Prune semantics confirmed solid: merge base is `origin/<default>`, branch refs always
  survive, and current/default/dirty/locked/detached worktrees are protected.
