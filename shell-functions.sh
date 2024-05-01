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
        cd $($CLONE "$1") || return
    fi
}

