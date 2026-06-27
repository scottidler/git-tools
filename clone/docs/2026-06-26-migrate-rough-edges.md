# Handoff: `clone --migrate` rough edges (found migrating tatari-tv/marquee)

**Date:** 2026-06-26
**Context:** Converted `~/repos/tatari-tv/marquee` from a flat checkout to the
bare-worktree layout with `clone --migrate`. It worked, but only after a
workaround, and the migration left several things for the operator to clean up
by hand. This doc is for a future session that will *improve* `--migrate`. It is
not a design doc; it is a punch list grounded in one real, messy migration.

Code under discussion: `clone/src/migrate.rs`, `clone/src/worktree.rs`,
`clone/src/lib.rs`. Design background already exists - do not re-derive it:
- `docs/design/2026-06-21-clone-bare-worktree.md`
- `docs/design/2026-06-21-clone-bare-worktree-implementation-notes.md`

---

## 1. BUG (blocker): relative `clonepath` breaks the post-swap `git worktree repair`

**Symptom (verbatim):**

```
$ cd ~/repos && clone --migrate tatari-tv/marquee
Error: migrated container failed repair/verification after swap
Caused by: repairing worktree links after swap
Caused by: git ["worktree", "repair", "./tatari-tv/marquee/main",
            "./tatari-tv/marquee/design-mermaid-diagrams"] exited 128:
  fatal: Invalid path '/home/saidler/repos/tatari-tv/marquee/tatari-tv':
  No such file or directory
```

**Root cause:** `flat` is built as `config.clonepath.join(spec)` (`lib.rs:39`).
With the default `clonepath = "."`, `flat` is the *relative* path
`./tatari-tv/marquee`. In `repair_worktrees` (`migrate.rs:246-255`) the worktree
names are computed as `container.join(name)` (still relative) and then
`git worktree repair <relative paths>` is run with `cwd = container`
(`git::run(&args, Some(container), ...)`). Git re-resolves the relative path
arguments against that cwd, producing `<container>/tatari-tv/marquee/main` - the
`tatari-tv/marquee` segment appears twice.

**Workaround used (so the operator isn't blocked):** pass an absolute clonepath.

```
cd ~/repos && clone --migrate --clonepath /home/saidler/repos tatari-tv/marquee
# -> /home/saidler/repos/tatari-tv/marquee/main   (success)
```

**Suggested fix:** canonicalize `flat` once at the top of `migrate_flat_to_bare`
(or absolutize in `Config::try_from` so every op gets an absolute target), so the
repair-path join and the `git worktree repair` cwd can never disagree. Add a
regression test that runs `--migrate` with a *relative* clonepath and a repo that
has at least two worktrees (the default + one more) - the existing tests passed
because they presumably used absolute temp paths.

**Good news - rollback works.** On the repair/verify failure the recoverable swap
fired exactly as designed (`migrate.rs:105-109`): it `rkvr rmrf`'d the broken
swapped-in container and renamed `marquee.backup` back. The original flat checkout
came back intact (right branch, clean tree, all 40 refs, all worktrees). Keep this
behavior; it is the reason a tool bug didn't cost any data.

---

## 2. `ensure_clean` checks only the main tree - linked worktrees can be silently stranded

`ensure_clean` (`migrate.rs:121-141`) checks `git status --porcelain` and
`git stash list` for the *flat repo's own* working tree only. The marquee flat
checkout had **3 sibling linked worktrees** (`../marquee-lostsignal`,
`../marquee-sccache`, `../marquee-slides`), and one of them (`slides`) had an
uncommitted `.otto.yml` change. `git clone --bare <flat>` only captures committed
refs, so that change would have been **lost without warning** if I hadn't found it
manually and committed it first.

**Suggested fix:** before migrating, enumerate `git worktree list --porcelain`,
and for each *linked* worktree run a dirtiness check; refuse (or at least loudly
warn) if any linked worktree has uncommitted/untracked changes. The current
"refuse on dirty" guarantee should cover the whole worktree set, not just HEAD's.

---

## 3. Linked worktrees are orphaned, not carried over

Migrate creates worktrees for the default branch and the previously-checked-out
branch only (`migrate.rs:71-86`). The 3 sibling worktrees' *branches* survived
(they are `refs/heads/*`, so the bare-clone-from-local kept them), but their
working directories became broken orphans: each `.git` file pointed at
`marquee.backup/.git/worktrees/<name>`, which migrate then deleted. I had to
recreate them by hand:

```
clone --worktree slides
clone --worktree add-status-error-page-simulator
clone --worktree docs-sccache-image-build-note
rkvr rmrf marquee-lostsignal marquee-sccache marquee-slides   # stale orphan dirs
```

**Suggested fix:** capture the set of existing linked worktrees up front and
recreate each as a native worktree in the new container after the swap (the
machinery already exists - `bare::add_worktree` / `worktree::add`). Optionally
`rkvr` the now-orphaned external dirs, or at minimum print the exact commands.

---

## 4. Stashes: refuse-guard is right, but there is no preservation path

`ensure_clean` refuses on a non-empty stash (correct - `git clone --bare` does
**not** copy `refs/stash`, so stashes would vanish). But the operator is left to
figure out preservation. Here I converted each stash to a `wip/*` branch
(`git branch wip/<name> stash@{N}` then `git stash drop`) so it survived as a
`refs/heads/*` ref. Two such branches now exist in marquee: `wip/gha-survey-autostash`,
`wip/park-readme`.

**Suggested fix:** when refusing on a non-empty stash, print the exact
"convert to branch" recipe, or offer a flag (e.g. `--rescue-stash`) that
auto-materializes each stash entry as a `wip/stash-N` branch before migrating.

---

## 5. Build-dir relocation (`target` symlink) is not handled

The flat checkout had `target -> /media/.../intel-480gb-ssd/cargo-target/tatari-tv/marquee/target`
(the relocate-targets setup). After migration none of the new worktrees have a
`target` symlink, so the next `cargo build` in any worktree will write to the OS
disk and defeat the SSD relocation until the relocate-targets cron catches it.
Out of scope for migrate itself, but worth a one-line note in `--migrate` output
("worktrees have no `target` symlink; run relocate-targets") or a cross-reference.

---

## 6. Documented-but-dropped machine-local state

`warn_dropped_state` (`migrate.rs:163-180`) already warns that custom hooks and
"extra .git/config remotes, alternates, reflogs" are not migrated. That warning
fired. It is honest, but for a heavily-used checkout (marquee had 40 local
branches, most unpushed) the reflog loss is the kind of thing worth calling out
more prominently than a single WARN line - consider summarizing what is being
left behind (e.g. "N reflog entries, M custom hooks").

---

## Current state of marquee (so the next session can verify, not redo)

`~/repos/tatari-tv/marquee` is now a healthy bare container:

```
marquee/.bare                              (bare; remote.origin.fetch fixed)
marquee/.git                               -> "gitdir: ./.bare" (relative)
marquee/main                               2ad0960 [main]        <- default
marquee/mcp                                2ad0960 [mcp]         <- new, the original ask
marquee/design-mermaid-diagrams            8c7e219 [design-mermaid-diagrams]
marquee/slides                             ebc34ff [slides]      (carries .otto.yml WIP)
marquee/add-status-error-page-simulator    b935280
marquee/docs-sccache-image-build-note      eee09a2
```

- `git fsck --connectivity-only` on `.bare` is clean (exit 0; dangling objects are
  the dropped-stash commits, also preserved as `wip/*` branches).
- Work identity verified in worktrees: `scott.idler@tatari.tv` (the
  `includeIf gitdir:~/repos/tatari-tv/` persona invariant still holds).

## Recovery / backout artifacts (rkvr)

- **Full pre-migration backup** (flat checkout + all 3 sibling worktrees, `.git`
  included, `target` stored as symlink not dereferenced):
  `rkvr ls-bkup` -> `/var/tmp/bkup/2026-06-26-170945-00{0..3}/`
- **Stale orphan sibling dirs** removed during cleanup:
  `/var/tmp/rmrf/2026-06-26-171640-00{0..2}/`
- Recover with `rkvr rcvr <path>`.

## How to reproduce the bug for testing the fix

Create a flat checkout with >=2 worktrees, then migrate from a parent dir using a
*relative* clonepath:

```
clone --flat <org>/<repo>            # or any existing flat checkout
cd <repo> && git worktree add ../<repo>-x -b x && cd ..
clone --migrate <org>/<repo>         # run from the PARENT, relative clonepath -> reproduces
```

Expect the `git worktree repair ... Invalid path '.../<org>': No such file or
directory` failure, followed by a correct rollback to the flat checkout.

## Suggested skills for the next session

- `/rust-cli-coder` - the fix lives in Rust (`migrate.rs` / `config.rs`); follow
  repo conventions.
- `/otto` - `otto ci` in `git-tools` to validate (lint + check + test).
- `/create-design-doc` then `/how-to-execute-a-plan` - if the worktree-set
  handling (items 2 + 3) grows beyond a point fix into a real behavior change.
- `/shipit` or `/bump` - git-tools releases via `bump` (single flat `v*` tag,
  whole workspace) then `otto install`; main is ungated for these Rust CLIs.
