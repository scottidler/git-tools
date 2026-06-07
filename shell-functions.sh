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

