# Contributing

This repository is a Rust port of Tendermint 0.34.42. Work that extends the protocol for Eld belongs in a separate project.

## Before you start

Open an issue. Use the bug, compatibility, or feature template.

A vulnerability, a validator key, or a node key does not belong in an issue. Report it in private. See [SECURITY.md](SECURITY.md).

## What a change may touch

- Rust that matches Tendermint 0.34.42.
- Tests, including hex checks against the Go encoding.
- `proto/` only when that Go schema changes. Regenerate with `scripts/gen-proto.sh`. Do not hand-edit `crates/proto/src/prost/`.

Do not edit `spec/`. It is a copy of the Go tree.

Do not commit a node home, `priv_validator_key.json`, `node_key.json`, or anything under `data/`.

## Checks

Rust 1.86.0, from `rust-toolchain.toml`.

```bash
./scripts/ci.sh
```

That is the same check GitHub runs: the Go-version pin, `cargo fmt --check`, Clippy, build, test, `cargo audit`, `cargo deny`, and gitleaks. Rustc and Clippy warnings are errors. The store tests build RocksDB, which needs CMake.

## Pull requests

The pull request template asks for three things:

- The Go reference commit you checked against.
- The tests you ran.
- Whether any encoding changed relative to the Go node.
