#!/usr/bin/env zsh

if [ -n "$DEBUG" ]; then
    PS4=':${LINENO}+'
    set -x
fi

CLONE=$(print -r -- =clone)

function clone() {
    if [[ "$@" == *"-h"* ]] || [[ "$@" == *"--help"* ]]; then
        eval $CLONE "$@"
    else
        cd $($CLONE $1)
    fi
}
