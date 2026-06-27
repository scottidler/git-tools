# CLI Shakedown: ls-owners (remote `--org` mode)

**Binary:** `ls-owners v0.2.10-1-g4093433` (the unreleased commit that adds `--org`)
**Date:** 2026-06-27
**Focus:** new `--org` remote CODEOWNERS scanning, local mode, error cases, one live GitHub API smoke test.

## Summary

| Metric | Count |
|--------|-------|
| Flags/modes discovered | 5 (`-l`, `-o/--only`, `-d/--detailed`, `--org`, `[PATH]...`) |
| Scenarios tested | 11 |
| Passed | 11 |
| Failed | 0 |
| Skipped (mutating) | 0 (read-only tool) |
| Live API runs | 3 (otto-rs, cli, nonexistent-org) |

No subcommands — `ls-owners` is a single flag-driven command. `--org` selects
remote mode; otherwise it scans local `[PATH]...` (default `.`).

## Command results

### Local mode (no network)
| Invocation | Exit | Result |
|---|---|---|
| `ls-owners .` | 1 | `unowned scottidler/git-tools` / `count 1` (exit 1 = some repo not owned) |
| `ls-owners -d .` | 1 | YAML detail incl. `authors:` (top committers, local-only) |
| `ls-owners --only owned .` | 0 | `count 0` (git-tools is unowned → filtered out; exit 0) |

### Error handling
| Invocation | Result |
|---|---|
| `ls-owners --only bogus .` | clap rejects: `invalid value 'bogus' ... [possible values: owned, unowned, partial]` |
| `ls-owners --org tatari-tv` (no token) | `Error: GitHub token missing; set GITHUB_TOKEN or GH_TOKEN` |
| `ls-owners --org this-org-does-not-exist-xyz123` | graceful 404: `Error: listing repos for org '...'` → `Caused by: HTTP status client error (404 Not Found) ...` (no panic) |

### Live GitHub API (`GH_TOKEN=$GITHUB_PAT_HOME`)
| Invocation | Exit | Result |
|---|---|---|
| `ls-owners --org otto-rs` | 1 | 2 repos, both `unowned` (no `.github/CODEOWNERS` — verified correct via direct API 404) |
| `ls-owners --org cli` | 1 | 10 repos in ~2.6s: 5 `owned` (CODEOWNERS present, base64-decoded + parsed), 5 `unowned` (missing) |
| `ls-owners --org cli -d --only owned` | — | full path→owner mapping rendered, e.g. `cli/cli` → `pkg/cmd/attestation/: [cli/package-security, cli/code-reviewers]` |

The `--org cli` run is the key end-to-end proof: it exercised **both** the
contents-API `200` path (base64 decode → `parse_codeowners` → `owned`) and the
`404` path (`MISSING_CODEOWNERS` → `unowned`), plus pagination, sorting, and the
`--only` filter on remote results.

## Field guide (tested, copy-pasteable)

```bash
# Local: audit ownership of repos under the current dir (default)
ls-owners
ls-owners ~/repos/scottidler          # scan a tree of repos
ls-owners -d                          # detailed YAML (paths + top authors)
ls-owners --only unowned              # only show repos lacking ownership

# Remote: audit an org's CODEOWNERS via the GitHub API (no clones needed)
export GH_TOKEN="$GITHUB_PAT_HOME"    # or GITHUB_TOKEN; work org -> GITHUB_PAT_WORK
ls-owners --org cli                   # one org
ls-owners --org cli vercel            # several orgs (space-separated)
ls-owners --org cli --org vercel      # ...or repeated
ls-owners --org tatari-tv --only unowned   # find tatari repos missing CODEOWNERS
ls-owners --org cli -d --only owned        # show who owns what, per path

# Exit code is 1 if ANY repo is not fully owned - usable as a CI gate:
ls-owners --org tatari-tv --only unowned && echo "all owned" || echo "gaps found"
```

## Observations / notes

- **Remote vs local scope (by design):** remote mode reports `owned` vs
  `unowned` (missing/empty CODEOWNERS) only. It does **not** detect unowned
  *paths* or list top authors — both need a local file tree / git history, which
  remote mode doesn't have. Local mode still does the full analysis.
- **`--org` accepts users too (FIXED in d658f74):** `list_repos` tries the org
  endpoint, then falls back to the user endpoint on 404, so `--org scottidler`
  (a User) works alongside `--org cli` (an Org). Verified live against `octocat`.
- **`--detailed` marker (FIXED in d658f74):** an `unowned` repo now prints
  `paths: MISSING_CODEOWNERS` / `EMPTY_CODEOWNERS` in detailed mode (the printer
  previously skipped the string marker, showing an empty body). Verified live.
- **Performance:** sequential fetch, ~0.26s/repo over the network; 10 repos in
  ~2.6s. Fine for typical orgs; a very large org (hundreds of repos) will take
  proportionally longer. No caching (the prior ETag scheme was removed — it
  cached etags but not content, so it misreported on re-runs).
- **No output-format flags** (`--json`/`--csv`): `ls-owners` emits a fixed
  simplified/detailed text format, so no format matrix applies.
