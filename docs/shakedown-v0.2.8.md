# CLI Shakedown Report: git-tools v0.2.8

Date: 2026-06-21. Host: Linux x86_64. All 7 workspace binaries installed at v0.2.8
via `otto install`. Emphasis on the freshly shipped `clone` bare-worktree features.

## Summary

| Metric | Count |
|--------|-------|
| Binaries discovered | 7 (each single-command, no subcommands) |
| Commands/flows tested | 30+ |
| Passed | all core flows |
| Failed | 0 crashes; 1 usability bug, 2 UX papercuts (below) |
| Skipped (mutating) | none — `clone` exercised in a `/tmp` sandbox |
| Pipelines tested | 2 |
| Edge cases tested | 8 |

Verdict: **clone's bare-worktree feature set works end-to-end** — every design behavior
(bare default, refspec fix, default-branch detection, `--flat`, `--worktree` branch-source
selection, `--migrate` with local-work preservation, the cd/z shim, all flag conflicts)
verified against both a local fixture and a real network clone. The six `ls-*`/`reposlug`
tools work; findings are UX-level, not correctness.

## clone v0.2.8 — bare-worktree (primary focus)

Sandbox: a local fixture origin (`testorg/testrepo` with `main` + `feature/auth`) plus one
real GitHub clone. Binary invoked via `command clone` to bypass the shell-function wrapper.

| # | Scenario | Invocation | Result |
|---|----------|-----------|--------|
| A | Bare default | `clone --clonepath W --remote R testorg/testrepo` | ✅ `.bare/` + `.git`→`gitdir: ./.bare` + `main/` worktree; stdout = `.../testorg/testrepo/main` |
| — | Refspec fix | (part of A) | ✅ `git branch -r` shows `origin/HEAD→origin/main`, `origin/feature/auth`, `origin/main` |
| B | Idempotent re-run | same command again | ✅ exit 0, reconciles, returns the worktree |
| C | `--worktree` new branch | `--worktree "Add Auth"` | ✅ slug `add-auth` used as **both** branch and dir |
| D | `--worktree` existing remote branch | `--worktree feature/auth` | ✅ dir slugified to `feature-auth`, branch keeps real name `feature/auth` |
| E | `--flat` legacy | `--flat --clonepath F ...` | ✅ `.git` is a directory, no `.bare` |
| F | `--migrate` flat→bare | `--migrate ...` on a flat clone with an unpushed commit + local-only branch | ✅ unpushed commit survived, local-only branch survived, origin repointed to real remote, HEAD reset to `main`, backup `rkvr`'d, no leftover dirs |
| G | cd/z navigation shim | `zsh -c 'source shell-functions.sh; cd <container>'` | ✅ redirects into `main/`; leaves worktrees and non-bare dirs alone |
| H | Flag conflicts | `--flat --worktree`, `--versioning --migrate`, `--worktree --migrate` | ✅ all rejected, exit 1, clear messages |
| I | Real network clone | `clone --clonepath N octocat/Hello-World` (ssh) | ✅ bare layout; default branch correctly detected as **`master`** (not hardcoded `main`) |

The Finding-1 migrate bug from the implementation audit (default branch wrongly taken from
the checked-out branch) is verified fixed: test F migrated a clean-on-main flat clone and the
HEAD reset to the true default; the audit's own regression test plus the live network clone
(default `master`) confirm remote-default detection.

## The other 6 tools

| Tool | Tested | Result |
|------|--------|--------|
| `reposlug` | flat repo, inside a bare worktree, non-repo dir | ✅ `scottidler/git-tools`; `octocat/Hello-World` from inside a worktree; clean error + exit 1 outside a repo |
| `ls-git-repos` | scan sandbox; `--lang rust`/`python` | ✅ each bare container = **one logical repo** (not per-worktree); `--lang` filters correctly |
| `ls-owners` | git-tools; `--only`, `--detailed` | ✅ detects unowned + author breakdown — see Findings for `--only` arg-ordering and exit code |
| `ls-stale-branches` | `30 git-tools`; missing/non-numeric DAYS | ✅ groups stale branches by author; clean usage/parse errors |
| `ls-github-repos` | `scottidler --repo-type user`; `--lang`, `--age`; bad name | ✅ lists/filters/age-annotates — see Findings for the token-required error message |
| `ls-stale-prs` | `30 git-tools`; missing DAYS | ✅ exit 0 (no open stale PRs); clean usage error |

## Output Format Matrix

**No tool exposes `--json` or `--csv`.** Output forms:

| Tool | Output |
|------|--------|
| reposlug | single `owner/repo` line |
| ls-git-repos | `owner/repo` lines |
| ls-github-repos | `owner/repo` lines (`--age` prefixes a date) |
| ls-owners | `<status> owner/repo` + `count N`; `--detailed` = YAML-style |
| ls-stale-branches | YAML-style (`repo:` → `author: (count, oldest_days)`) |
| ls-stale-prs | YAML-style |

Pipelines therefore use plain-text composition (`sort`/`uniq`/`wc`/`grep`), not `jq`.

## Failures & Bugs

1. **`ls-owners --only <status> <path>` — variadic flag swallows the path** (usability bug).
   `--only` is multi-valued, so `ls-owners --only unowned /path` parses `/path` as a second
   `--only` value: `error: invalid value '/path' for '--only <FILTER>...'`. Workarounds (both
   verified): put the path first (`ls-owners /path --only unowned`) or use `--`
   (`ls-owners --only unowned -- /path`). Fix options: make `--only` a repeatable single-value
   option, or document the ordering.

2. **`ls-github-repos` misleading error for an unconfigured/nonexistent name** (UX, minor).
   `resolve_token` (`ls-github-repos/src/main.rs:80-103`) requires a token for every call (YAML
   config name→env-var, else a per-name file `<token-path>/<name>`). A typo'd or token-less
   name surfaces `Error: Failed to read token file '~/.config/github/tokens/<name>'` rather than
   "no token configured for '<name>'" or "user/org not found". (`scottidler` works because it
   has a configured token — the tool is auth-required, not unauthenticated.)

3. **eyre `Location:` line on user-facing CLI errors** (cosmetic). e.g. `clone`'s flag-conflict
   errors print `Location: clone/src/config.rs:87:24` under the message. Accurate but
   dev-oriented; a user-facing CLI typically suppresses the location for clap/validation errors.

## Pipeline Recipes (verified)

```bash
# Count scottidler's Rust repos (-> 55)
ls-github-repos scottidler --repo-type user --lang rust | wc -l

# Group discovered repos; bare containers dedupe to one logical repo each
ls-git-repos ~/repos | sort | uniq -c

# Slug of the repo you're standing in (works inside a bare worktree too)
reposlug .

# Stale-branch sweep across several repos
ls-stale-branches 90 ~/repos/scottidler/git-tools ~/repos/tatari-tv/<repo>
```

## Edge Cases (all handled gracefully)

| Input | Behavior |
|-------|----------|
| `clone` (no args) | help, exit 2 |
| `clone --flat --worktree` / `--versioning --migrate` / `--worktree --migrate` | clear conflict error, exit 1 |
| `reposlug /tmp` (non-repo) | "could not parse remote" error, exit 1 |
| `ls-stale-branches` (no DAYS) | usage, non-zero |
| `ls-stale-branches abc <path>` | "invalid digit found in string", exit 2 |
| `ls-github-repos <nonexistent>` | token-file error (see Finding 2) |
| `clone --worktree` non-bare container | "not a bare container; … run `clone --migrate` first" |
| commitless/empty remote | (covered by unit tests) skip worktree add, land in container |

## Release Validation

- **Tag `v0.2.8`:** annotated (`git cat-file -t` → `tag`), points at `d4220e2` == `origin/main`. ✅
- **Workflow `binary-release`:** completed, conclusion **success** (run 27917892426). ✅
- **Assets:** `git-tools-v0.2.8-linux.tar.gz` (8.3 MB) — bundles all 7 binaries + `shell-functions.sh`.
  - Note: **Linux-only** single bundle; no `aarch64`/`darwin` targets. This is the repo's release
    design, not a regression — but cross-platform consumers have no asset.
- **Binary test:** downloaded the tarball, extracted, ran `./clone --version` → `clone v0.2.8`,
  matching the locally installed binary. ✅

## Observations

- The bare-worktree work is solid and matches the design doc + the post-implementation audit fixes.
- Consider a shared `--json` output across the `ls-*` tools — the YAML-ish/plain formats are
  pipe-friendly for humans but awkward for machine consumption (no structured field access).
- The `clone` shell function shadows the binary name; binary-level testing needs `command clone`
  (or `=clone`). Worth a note in CLAUDE.md for anyone scripting against the binary directly.
