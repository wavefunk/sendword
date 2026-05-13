#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    set -- sendword serve
fi

command_name=${1##*/}
if [ "$command_name" = "sendword" ]; then
    subcommand=${2:-serve}
    if [ "$subcommand" = "serve" ] && [ ! -e data ]; then
        ln -s . data
    fi
fi

exec "$@"
