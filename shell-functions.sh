#!/usr/bin/env zsh

if [ -n "$DEBUG" ]; then
    PS4=':${LINENO}+'
    set -x
fi

CLONE=$(print -r -- =clone)

clone() {
    if [[ "$1" == (-h|--help|-v|--version) ]]; then
        eval $CLONE "$@"
    else
        local dest
        dest=$(eval $CLONE "$@") || return $?
        if [[ -z "$dest" || ! -d "$dest" ]]; then
            print -u2 -- "clone: no valid destination returned; staying in $PWD"
            return 1
        fi
        cd "$dest"
    fi
}

WORKTREE=$(print -r -- =worktree)

# `worktree <branch>` switches to (or creates) that worktree, and `worktree`
# with no arg opens an fzf picker over the container's worktrees -- both print
# the chosen path to stdout and the function cd's into it (the same
# binary-prints-path / function-does-the-cd contract `clone` uses). Flags
# (`--list`/`-L`, `-h`, `--version`) run straight through: their stdout is for
# you, not a path to cd into. A branch never starts with `-`, so the dispatch is
# unambiguous.
worktree() {
    case "$1" in
        -*)
            eval $WORKTREE "$@"
            ;;
        *)
            local dest
            dest=$(eval $WORKTREE "$@") || return $?
            if [[ -z "$dest" || ! -d "$dest" ]]; then
                print -u2 -- "worktree: no valid destination returned; staying in $PWD"
                return 1
            fi
            cd "$dest"
            ;;
    esac
}

