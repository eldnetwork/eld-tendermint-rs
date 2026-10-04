#!/bin/sh
set -e
if [ "$1" = "eld-tendermint-rs" ] && [ "$2" = "start" ] && [ -n "${PROXY_APP}" ]; then
  shift 2
  exec eld-tendermint-rs start --proxy-app="${PROXY_APP}" "$@"
fi
exec "$@"
