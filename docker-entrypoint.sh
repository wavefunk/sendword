#!/bin/sh
set -eu

if [ "$#" -eq 0 ]; then
    set -- sendword serve
fi

if [ "$1" = "sendword" ] && [ "${2:-}" = "serve" ] && [ ! -e data ]; then
    ln -s . data
fi

exec "$@"
