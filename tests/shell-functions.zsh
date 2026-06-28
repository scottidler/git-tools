#!/usr/bin/env zsh
# Tests for the `clone` shell-function wrapper in ../shell-functions.sh.
#
# Sources the REAL shell-functions.sh (not a copy, so the test can't drift from
# the shipped wrapper) and exercises the wrapper contract that keeps a failed
# clone from silently `cd`-ing you to $HOME (the bug fixed in v0.2.5):
#
#   * the binary prints the destination path to stdout, errors to stderr,
#     and exits non-zero on failure;
#   * the function captures stdout, bails on a non-zero exit BEFORE any `cd`,
#     and guards against empty / non-directory output.
#
# The bare-container `chpwd` navigation shim was removed (it intercepted every
# `cd` and stranded you on the bare root); navigation now lives in a dedicated
# worktree tool, not in this wrapper. There is nothing chpwd-related left to test.

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

# A stub `clone` on PATH whose behavior we drive per-test via env vars:
#   STUB_OUT  - what it prints to stdout (the "destination path")
#   STUB_RC   - the exit code it returns
# This lets us model success, failure, and malformed output without the real
# binary or network.
stubbin=$root/stubbin
mkdir -p "$stubbin"
cat > "$stubbin/clone" <<'STUB'
#!/usr/bin/env zsh
[[ -n "$STUB_OUT" ]] && print -r -- "$STUB_OUT"
exit ${STUB_RC:-0}
STUB
chmod +x "$stubbin/clone"

# A stub `worktree` on PATH, driven the same way. shell-functions.sh captures
# `=worktree` at source time, so this must exist before the source below.
# When $STUB_ARGV_FILE is set it records argc on the first line and one arg per
# line after, so a test can prove the wrapper forwards args without re-splitting
# (the eval word-split bug: `worktree "New Feature"` must arrive as ONE arg).
cat > "$stubbin/worktree" <<'STUB'
#!/usr/bin/env zsh
if [[ -n "$STUB_ARGV_FILE" ]]; then
    print -r -- "$#" > "$STUB_ARGV_FILE"
    for a in "$@"; do print -r -- "$a" >> "$STUB_ARGV_FILE"; done
fi
[[ -n "$STUB_OUT" ]] && print -r -- "$STUB_OUT"
exit ${STUB_RC:-0}
STUB
chmod +x "$stubbin/worktree"
export PATH=$stubbin:$PATH

dest=$root/repos/org/repo/main
mkdir -p "$dest"
home=$root/home          # a stand-in $HOME to prove we never land here on failure
mkdir -p "$home"

# --- source the shipped wrapper ----------------------------------------------
source "$SHELL_FUNCS"

# --- scenarios ---------------------------------------------------------------

# Success: binary prints a real dir, exits 0 -> wrapper cd's into it.
builtin cd "$home"
STUB_OUT=$dest STUB_RC=0 clone org/repo
check "success -> cd into the printed destination" "$dest" "$PWD"

# Failure: binary exits non-zero -> wrapper bails BEFORE cd, stays put, non-zero.
builtin cd "$home"
STUB_OUT="$dest" STUB_RC=1 clone org/repo 2>/dev/null
rc=$?
check "binary non-zero -> wrapper returns non-zero" "1" "$rc"
check "binary non-zero -> did NOT cd (stayed put)"  "$home" "$PWD"

# Empty stdout (even with rc 0) -> guarded, no cd, non-zero.
builtin cd "$home"
STUB_OUT="" STUB_RC=0 clone org/repo 2>/dev/null
rc=$?
check "empty stdout -> wrapper returns non-zero" "1" "$rc"
check "empty stdout -> did NOT cd (stayed put)"  "$home" "$PWD"

# Non-directory stdout -> guarded, no cd, non-zero.
builtin cd "$home"
STUB_OUT="$root/does/not/exist" STUB_RC=0 clone org/repo 2>/dev/null
rc=$?
check "non-dir stdout -> wrapper returns non-zero" "1" "$rc"
check "non-dir stdout -> did NOT cd (stayed put)"  "$home" "$PWD"

# Help/version pass straight through (no path capture, no cd attempt).
builtin cd "$home"
STUB_OUT="ignored" STUB_RC=0 clone --help >/dev/null 2>&1
check "--help passes through -> no cd" "$home" "$PWD"

# --- worktree() dispatch -----------------------------------------------------
# `worktree <branch>` AND the no-arg picker both capture stdout and cd into the
# printed path; flag forms (`--list`, `-h`, `--version`) pass straight through
# and never cd. A branch never starts with `-`, so the `-*` vs `*` split is
# unambiguous.

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

# A branch name with a space must reach the binary as a SINGLE arg, not two
# (regression: `eval $WORKTREE "$@"` word-split "New Feature" into New + Feature).
builtin cd "$home"
argv_file=$root/worktree.argv
STUB_OUT=$dest STUB_RC=0 STUB_ARGV_FILE=$argv_file worktree "New Feature"
check "worktree 'New Feature' -> binary got 1 arg"        "1"           "$(sed -n 1p "$argv_file")"
check "worktree 'New Feature' -> arg preserved verbatim"  "New Feature" "$(sed -n 2p "$argv_file")"

# --- result ------------------------------------------------------------------
if (( fails )); then
    print -u2 -- "\n$fails test(s) failed"
    exit 1
fi
print -- "\nall shell-functions tests passed"
