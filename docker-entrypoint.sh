#!/bin/sh
set -e
if [ "$1" = "eld-tendermint" ] && [ "$2" = "start" ] && [ -n "${PROXY_APP}" ]; then
  shift 2
  exec eld-tendermint start --proxy-app="${PROXY_APP}" "$@"
fi
exec "$@"
