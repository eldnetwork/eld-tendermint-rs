# Contributing

This repository is a Rust port of Tendermint Core v0.34.24. Work that extends the protocol for Eld belongs in a separate project.

## Before you start

Open an issue. Use the bug, compatibility, or feature template.

A vulnerability, a validator key, or a node key does not belong in an issue. Report it in private. See [SECURITY.md](SECURITY.md).

## What a change may touch

- Rust that matches Tendermint Core v0.34.24.
- Tests, including hex checks against the Go encoding.
- `proto/` only when that Go schema changes. Regenerate with `scripts/gen-proto.sh`. Do not hand-edit `crates/proto/src/prost/`.

Do not edit `spec/` or the `.proto` files. They are vendored verbatim from the commit in `proto/GO_REF`.

Do not commit a node home, `priv_validator_key.json`, `node_key.json`, or anything under `data/`.

## Checks

Rust 1.86.0, from `rust-toolchain.toml`.

```bash
./scripts/ci.sh
```

That is the same check GitHub runs: the Go-version pin, `cargo fmt --check`, Clippy, build, test, `cargo audit`, `cargo deny`, and gitleaks. Rustc and Clippy warnings are errors. The store tests build RocksDB, which needs CMake.

## Dependency updates

Dependabot opens pull requests for Cargo crates and for GitHub Actions. It does not merge them. Review the diff. `./scripts/ci.sh` has to pass, including `cargo audit` and `cargo deny`.

A RustSec alert is its own pull request. Other Cargo updates are one weekly pull request, and other Actions updates are another. Read the lockfile diff. A change in cryptography, encoding, or the database crate needs the same care as any other change to the wire format.

These stay manual. Dependabot does not bump them:

- `cargo-audit` and `cargo-deny` in `.github/workflows/ci.yml`. The workflow checks each download against a SHA-256. Change the version and the hash together.
- `gitleaks` in that same file. The version is pinned. Change it by hand.
- Rust 1.86.0. Change `rust-toolchain.toml` and `dtolnay/rust-toolchain@1.86.0` in the workflow together.

## Pull requests

The pull request template asks for three things:

- The Go reference commit you checked against.
- The tests you ran.
- Whether any encoding changed relative to the Go node.
