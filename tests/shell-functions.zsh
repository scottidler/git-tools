#!/usr/bin/env zsh
# Tests for the bare-container navigation shim in ../shell-functions.sh.
#
# Sources the REAL shell-functions.sh (not a copy, so the test can't drift from
# the shipped hook) and exercises the chpwd redirect against a real bare
# container. Focus: the directional carve-out -- ascending into a container from
# one of its own worktrees parks you on the bare root, while arriving from
# outside still redirects into the default-branch worktree.

emulate -L zsh
export DEBUG=          # shell-functions.sh probes $DEBUG at source time

SCRIPT_DIR=${0:A:h}
SHELL_FUNCS=$SCRIPT_DIR/../shell-functions.sh

fails=0
check() {
    local desc=$1 expected=$2 actual=$3
    if [[ "$actual" == "$expected" ]]; then
        print -- "ok   - $desc"
    else
        print -u2 -- "FAIL - $desc"
        print -u2 -- "         expected: $expected"
        print -u2 -- "         actual:   $actual"
        (( fails++ ))
    fi
}

# --- sandbox -----------------------------------------------------------------
root=$(mktemp -d); root=${root:A}
cleanup() {
    builtin cd /
    command -v rkvr >/dev/null 2>&1 && rkvr rmrf "$root" >/dev/null 2>&1
}
trap cleanup EXIT

# A stub `clone` on PATH so the top-level `CLONE=$(print -r -- =clone)` in
# shell-functions.sh resolves even where the real binary isn't installed (CI).
stubbin=$root/stubbin
mkdir -p "$stubbin"
print -- '#!/bin/sh\nexit 0' > "$stubbin/clone"
chmod +x "$stubbin/clone"
export PATH=$stubbin:$PATH

# A real bare container: .bare/ (HEAD -> main) + .git pointer + a main/ worktree.
container=$root/repos/org/repo
mkdir -p "$container/main" "$root/elsewhere" "$root/repos/org/other"
git init --bare -q "$container/.bare"
git --git-dir="$container/.bare" symbolic-ref HEAD refs/heads/main
print -- 'gitdir: ./.bare' > "$container/.git"

# --- source the shipped hook (registers _clone_chpwd_bare in chpwd_functions) -
source "$SHELL_FUNCS"

# --- scenarios ---------------------------------------------------------------

# Arriving from OUTSIDE redirects into the default-branch worktree.
builtin cd "$root/elsewhere"
cd "$container"
check "arrive from outside -> redirected into worktree" "$container/main" "$PWD"

# Arriving from a SIBLING dir also counts as outside -> redirect.
builtin cd "$root/repos/org/other"
cd "$container"
check "arrive from sibling -> redirected into worktree" "$container/main" "$PWD"

# Directional carve-out: `cd ..` UP from the worktree parks on the bare root.
builtin cd "$container/main"
cd ..
check "cd .. up from worktree -> stays on bare root" "$container" "$PWD"

# Carve-out holds at depth: ascending from a worktree subdir straight to root.
mkdir -p "$container/main/deep"
builtin cd "$container/main/deep"
cd ../..
check "cd ../.. up from worktree subdir -> stays on bare root" "$container" "$PWD"

# Escape hatch still works: cdbare from outside parks on the root, no redirect.
builtin cd "$root/elsewhere"
cdbare "$container"
check "cdbare from outside -> parks on bare root" "$container" "$PWD"

# --- result ------------------------------------------------------------------
if (( fails )); then
    print -u2 -- "\n$fails test(s) failed"
    exit 1
fi
print -- "\nall shell-functions tests passed"
