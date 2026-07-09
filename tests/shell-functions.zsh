#!/usr/bin/env zsh
# Tests for the `clone` and `worktree` shell-function wrappers emitted by the
# binaries themselves via `<bin> shell-init zsh`.
#
# There is no longer a static shell-functions.sh: each binary is the single
# source of truth for its own wrapper. This test `eval`s the emitted functions
# (exactly as `.zshrc` does) and exercises the wrapper contract that keeps a
# failed clone from silently `cd`-ing you to $HOME (the bug fixed in v0.2.5):
#
#   * the binary prints the destination path to stdout, errors to stderr,
#     and exits non-zero on failure;
#   * the function captures stdout, bails on a non-zero exit BEFORE any `cd`,
#     and guards against empty / non-directory output.
#
# The emitted body uses `command clone` / `command worktree`, which resolve the
# on-PATH binary at call time. We exploit that here: a stub `clone`/`worktree`
# placed first on PATH is what `command <bin>` resolves to, so the contract is
# driven without the real binary or network.
#
# The bare-container `chpwd` navigation shim was removed (it intercepted every
# `cd` and stranded you on the bare root); navigation now lives in the dedicated
# worktree tool, not in this wrapper. There is nothing chpwd-related left to test.

emulate -L zsh

SCRIPT_DIR=${0:A:h}
REPO_ROOT=${SCRIPT_DIR:h}

# Locate the built binaries the same way the workspace builds them: prefer a
# release build, fall back to debug. The CI `test`/`check` tasks compile the
# workspace before `shell-test` runs, so target/debug exists.
find_bin() {
    local name=$1
    local rel=$REPO_ROOT/target/release/$name
    local dbg=$REPO_ROOT/target/debug/$name
    if [[ -x "$rel" ]]; then
        print -r -- "$rel"
    elif [[ -x "$dbg" ]]; then
        print -r -- "$dbg"
    else
        print -u2 -- "FATAL - could not find a built '$name' binary under target/{release,debug}"
        print -u2 -- "         run 'cargo build -p clone -p worktree' first"
        exit 1
    fi
}

CLONE_BIN=$(find_bin clone)
WORKTREE_BIN=$(find_bin worktree)

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

# Stub `clone`/`worktree` on PATH whose behavior we drive per-test via env vars:
#   STUB_OUT  - what it prints to stdout (the "destination path")
#   STUB_RC   - the exit code it returns
#   STUB_ARGV_FILE - if set, records argc on line 1 then one arg per line, so a
#                    test can prove the wrapper forwards args without re-splitting
#                    (the eval word-split bug: `worktree "New Feature"` must
#                    arrive as ONE arg).
# The emitted functions call `command clone` / `command worktree`, which resolve
# these stubs because $stubbin is first on PATH.
stubbin=$root/stubbin
mkdir -p "$stubbin"
write_stub() {
    cat > "$stubbin/$1" <<'STUB'
#!/usr/bin/env zsh
if [[ -n "$STUB_ARGV_FILE" ]]; then
    print -r -- "$#" > "$STUB_ARGV_FILE"
    for a in "$@"; do print -r -- "$a" >> "$STUB_ARGV_FILE"; done
fi
[[ -n "$STUB_OUT" ]] && print -r -- "$STUB_OUT"
exit ${STUB_RC:-0}
STUB
    chmod +x "$stubbin/$1"
}
write_stub clone
write_stub worktree
export PATH=$stubbin:$PATH

dest=$root/repos/org/repo/main
mkdir -p "$dest"
home=$root/home          # a stand-in $HOME to prove we never land here on failure
mkdir -p "$home"

# --- define the wrappers from the EMITTED scripts (exactly as .zshrc does) ----
eval "$("$CLONE_BIN" shell-init zsh)"
eval "$("$WORKTREE_BIN" shell-init zsh)"

# Sanity: the emitted bodies (not some stale static file) are what we loaded.
# The body invokes the on-PATH binary via `command <bin>` (no `$CLONE`/`$WORKTREE`
# env var snapshot, which is what the retired static file used).
functions clone    | grep -q 'command clone';    check "clone() body uses 'command clone'"       "0" "$?"
functions worktree | grep -q 'command worktree';  check "worktree() body uses 'command worktree'" "0" "$?"
functions clone    | grep -q '\$CLONE';    check "clone() body drops the \$CLONE snapshot"        "1" "$?"
functions worktree | grep -q '\$WORKTREE'; check "worktree() body drops the \$WORKTREE snapshot"  "1" "$?"

# --- clone() scenarios -------------------------------------------------------

# Success: binary prints a real dir, exits 0 -> wrapper cd's into it.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 clone org/repo
check "clone success -> cd into the printed destination" "$dest" "$PWD"

# Failure: binary exits non-zero -> wrapper bails BEFORE cd, stays put, non-zero.
builtin cd "$home"
STUB_OUT="$dest" STUB_RC=1 clone org/repo 2>/dev/null
rc=$?
check "clone non-zero -> wrapper returns non-zero" "1" "$rc"
check "clone non-zero -> did NOT cd (stayed put)"  "$home" "$PWD"

# Empty stdout (even with rc 0) -> guarded, no cd, non-zero.
builtin cd "$home"
STUB_OUT="" STUB_RC=0 clone org/repo 2>/dev/null
rc=$?
check "clone empty stdout -> wrapper returns non-zero" "1" "$rc"
check "clone empty stdout -> did NOT cd (stayed put)"  "$home" "$PWD"

# Non-directory stdout -> guarded, no cd, non-zero.
builtin cd "$home"
STUB_OUT="$root/does/not/exist" STUB_RC=0 clone org/repo 2>/dev/null
rc=$?
check "clone non-dir stdout -> wrapper returns non-zero" "1" "$rc"
check "clone non-dir stdout -> did NOT cd (stayed put)"  "$home" "$PWD"

# Help/version pass straight through (no path capture, no cd attempt).
builtin cd "$home"
STUB_OUT="ignored" STUB_RC=0 clone --help >/dev/null 2>&1
check "clone --help passes through -> no cd" "$home" "$PWD"

# `clone shell-init` passes straight through too (must NOT be captured + cd'd).
builtin cd "$home"
STUB_OUT="ignored" STUB_RC=0 clone shell-init zsh >/dev/null 2>&1
check "clone shell-init passes through -> no cd" "$home" "$PWD"

# --- worktree() dispatch -----------------------------------------------------
# `worktree <branch>` AND the no-arg picker both capture stdout and cd into the
# printed path; flag forms (`--list`, `-h`, `--version`) and `shell-init` pass
# straight through and never cd. A branch never starts with `-`, so the `-*` vs
# `*` split is unambiguous.

# A branch arg with a real dir on stdout -> cd into it.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree feature
check "worktree <branch> -> cd into printed path" "$dest" "$PWD"

# A branch arg, binary fails -> bail before cd, non-zero.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=1 worktree feature 2>/dev/null
rc=$?
check "worktree <branch> non-zero -> returns non-zero" "1" "$rc"
check "worktree <branch> non-zero -> did NOT cd"       "$home" "$PWD"

# A branch arg, non-directory stdout -> guarded, no cd, non-zero.
builtin cd "$home"
STUB_OUT="$root/nope" STUB_RC=0 worktree feature 2>/dev/null
rc=$?
check "worktree <branch> non-dir -> returns non-zero" "1" "$rc"
check "worktree <branch> non-dir -> did NOT cd"       "$home" "$PWD"

# No arg (picker) captures the chosen path and cd's into it.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree
check "worktree (no arg, picker) -> cd into selection" "$dest" "$PWD"

# No arg, picker cancelled (non-zero) -> bail before cd, stay put.
builtin cd "$home"
STUB_OUT="" STUB_RC=130 worktree 2>/dev/null
check "worktree (no arg) cancelled -> did NOT cd" "$home" "$PWD"

# --list passes straight through and never cd's, even with a path on stdout.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree --list >/dev/null 2>&1
check "worktree --list -> passthrough, no cd" "$home" "$PWD"

# A flag passes through and never cd's, even with a path on stdout.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree --help >/dev/null 2>&1
check "worktree --flag -> passthrough, no cd" "$home" "$PWD"

# `worktree shell-init` passes straight through too (must NOT be captured + cd'd).
builtin cd "$home"
STUB_OUT="ignored" STUB_RC=0 worktree shell-init zsh >/dev/null 2>&1
check "worktree shell-init passes through -> no cd" "$home" "$PWD"

# A branch name with a space must reach the binary as a SINGLE arg, not two
# (regression: word-splitting "New Feature" into New + Feature).
builtin cd "$home"
argv_file=$root/worktree.argv
STUB_OUT=$dest STUB_RC=0 STUB_ARGV_FILE=$argv_file worktree "New Feature"
check "worktree 'New Feature' -> binary got 1 arg"        "1"           "$(sed -n 1p "$argv_file")"
check "worktree 'New Feature' -> arg preserved verbatim"  "New Feature" "$(sed -n 2p "$argv_file")"

# --- worktree() acquisition verbs (init/migrate/flatten) --------------------
# `init`/`migrate`/`flatten` are non-`-*` `argv[1]` tokens (reserved-word
# positionals, pre-clap dispatched in `worktree/src/main.rs`), so they must land
# in the SAME capture-and-cd branch as a bare branch name or the no-arg picker -
# never the `-*` passthrough. The stub doesn't care what the verb means; it only
# proves the wrapper's dispatch takes the cd path for these tokens.

# `worktree init <spec>` -> cd into the printed default worktree.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree init org/repo
check "worktree init -> cd into printed default worktree" "$dest" "$PWD"

# `worktree migrate [spec]` -> cd into the printed path.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree migrate org/repo
check "worktree migrate -> cd into printed path" "$dest" "$PWD"

# `worktree flatten [spec]` -> cd into the printed path.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree flatten org/repo
check "worktree flatten -> cd into printed path" "$dest" "$PWD"

# `worktree init`/`migrate`/`flatten`, binary fails -> bail before cd, non-zero
# (same failure contract as the branch/picker forms).
builtin cd "$home"
STUB_OUT=$dest STUB_RC=1 worktree init org/repo 2>/dev/null
rc=$?
check "worktree init non-zero -> returns non-zero" "1" "$rc"
check "worktree init non-zero -> did NOT cd"       "$home" "$PWD"

# `worktree migrate --dry-run` / `worktree flatten --dry-run`: per Phase 4, the
# binary prints its preview to STDERR ONLY and leaves stdout empty (worktree's
# `dry_run` -> `Outcome::Previewed`, never printed by main.rs), so the wrapper's
# existing empty-output guard bails BEFORE any cd, leaving the user in $PWD.
builtin cd "$home"
STUB_OUT="" STUB_RC=0 worktree migrate --dry-run org/repo 2>/dev/null
rc=$?
check "worktree migrate --dry-run -> wrapper returns non-zero" "1"     "$rc"
check "worktree migrate --dry-run -> did NOT cd (stayed put)" "$home" "$PWD"

builtin cd "$home"
STUB_OUT="" STUB_RC=0 worktree flatten --dry-run org/repo 2>/dev/null
rc=$?
check "worktree flatten --dry-run -> wrapper returns non-zero" "1"     "$rc"
check "worktree flatten --dry-run -> did NOT cd (stayed put)" "$home" "$PWD"

# A `--help`/`-V` AFTER a verb must pass through (no capture, no cd) so the
# binary's usage/version reaches the user instead of being swallowed by the
# empty-destination guard. Regression: `worktree init --help` used to hit the
# capture branch ($1=init) and print "no valid destination; staying in $PWD".
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree init --help >/dev/null 2>&1
check "worktree init --help -> passthrough, no cd" "$home" "$PWD"

builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 worktree migrate -V >/dev/null 2>&1
check "worktree migrate -V -> passthrough, no cd" "$home" "$PWD"

# Same class for clone: a trailing `--help` after a spec must pass through.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 clone org/repo --help >/dev/null 2>&1
check "clone <spec> --help -> passthrough, no cd" "$home" "$PWD"

# --- result ------------------------------------------------------------------
if (( fails )); then
    print -u2 -- "\n$fails test(s) failed"
    exit 1
fi
print -- "\nall shell-functions tests passed"
