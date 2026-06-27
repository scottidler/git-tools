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
#
# Directional carve-out: arriving at a container from OUTSIDE (a fresh
# `cd ~/repos/org/repo`, `z repo`, an IDE jump) redirects into the worktree, as
# before. But stepping UP into the container from one of its own worktrees
# (`cd ..` out of `<repo>/main`) is a deliberate move to the bare root -- honor
# it. We tell the two apart with $OLDPWD: if the previous dir is a descendant of
# the container root, we ascended from within, so don't bounce back.
#
# Escape hatch: set NO_BARE_REDIRECT (any non-empty value) to suppress the
# redirect for one directory change -- e.g. to land on the bare container root
# itself when arriving from outside. `cdbare` (below) does this for you. Because
# zsh hooks are dynamically scoped, a `local NO_BARE_REDIRECT=1` in the caller is
# visible here, so the suppression is scoped to that single cd and never leaks.
_clone_chpwd_bare() {
    [[ -n "$NO_BARE_REDIRECT" ]] && return
    [[ -d .bare && -f .git ]] || return
    [[ -n "$OLDPWD" && "$OLDPWD" == "$PWD"/* ]] && return
    local branch
    branch=$(git --git-dir=.bare symbolic-ref --short HEAD 2>/dev/null) || return
    [[ -n "$branch" && -d "$branch" ]] && builtin cd -- "$branch"
}

# cd to a bare container ROOT without being redirected into its default-branch
# worktree. `cdbare` (no arg) parks you on the container root of wherever you
# are; `cdbare <path>` cds there first. Needed because the chpwd shim otherwise
# bounces every `cd` into a bare container straight back into the worktree.
cdbare() {
    local NO_BARE_REDIRECT=1
    builtin cd -- "${1:-$PWD}"
}

typeset -ga chpwd_functions
if (( ! ${chpwd_functions[(I)_clone_chpwd_bare]} )); then
    chpwd_functions+=(_clone_chpwd_bare)
fi

