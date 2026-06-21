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

# Bare-container navigation shim. The `clone` wrapper already lands you in a
# fresh clone's default worktree, but every *other* way of arriving at a repo
# (cd ~/repos/org/repo, z repo, an IDE bookmark) would drop you in the bare
# container with no working files. This chpwd hook fires after any directory
# change (cd, pushd, z/zoxide) and, when the new directory is a bare container
# (a .bare/ dir + a .git pointer file), redirects into its default-branch
# worktree -- so you keep landing "in the repo," now meaning its worktree.
_clone_chpwd_bare() {
    [[ -d .bare && -f .git ]] || return
    local branch
    branch=$(git --git-dir=.bare symbolic-ref --short HEAD 2>/dev/null) || return
    [[ -n "$branch" && -d "$branch" ]] && builtin cd -- "$branch"
}

typeset -ga chpwd_functions
if (( ! ${chpwd_functions[(I)_clone_chpwd_bare]} )); then
    chpwd_functions+=(_clone_chpwd_bare)
fi

