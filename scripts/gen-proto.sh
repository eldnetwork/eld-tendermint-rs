#!/usr/bin/env bash
# Regenerate crates/proto/src/prost from proto/tendermint.
# Requires protoc on PATH. cargo test does not run this script.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo run --quiet --manifest-path tools/proto-compiler/Cargo.toml
