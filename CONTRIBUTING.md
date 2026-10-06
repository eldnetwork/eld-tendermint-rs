# Contributing

This repository is a Rust port of Tendermint Core v0.34.24. Work that extends the protocol for Eld belongs in a separate project.

## Before you start

Open an issue. Use the bug, compatibility, or feature template.

A vulnerability, a validator key, or a node key does not belong in an issue. Report it in private. See [SECURITY.md](SECURITY.md).

## What a change may touch

- Rust that matches Tendermint Core v0.34.24.
- Tests, including hex checks against the Go encoding.
- `proto/` only when that Go schema changes. Regenerate with `scripts/gen-proto.sh`. Do not hand-edit `crates/proto/src/prost/`.

Do not edit `spec/` or the `.proto` files. They are vendored verbatim from the commit in `proto/GO_REF`. The only local edit allowed in `spec/` is the note at the top of `spec/README.md`.

Do not commit a node home, `priv_validator_key.json`, `node_key.json`, or anything under `data/`.

## Checks

Rust 1.86.0, from `rust-toolchain.toml`.

```bash
./scripts/ci.sh
```

That is the same check GitHub runs: the Go-version pin, the vector check, `cargo fmt --check`, Clippy, build, test, `cargo audit`, `cargo deny`, and gitleaks. Rustc and Clippy warnings are errors. Clippy also denies `.unwrap()`, `.expect()`, `dbg!`, and `todo!` outside tests. Tests may use unwrap, expect, and `dbg!`. A length or lock that cannot fail uses `.ensured()`, which panics if the invariant is broken. A path that can fail returns `Result`. Formatting is rustfmt's defaults. There is no `rustfmt.toml`. The store tests build RocksDB, which needs CMake.

Library crates return their own `Error` enum and keep the source error. The `eld-tendermint` binary may collapse that into a string before it exits. Do not add `anyhow` to a library crate.

`unsafe` is forbidden outside `crates/config`. The host-name lookup in `crates/config/src/hostname.rs` is the only `unsafe` block. RocksDB is opened from `crates/store/src/db.rs`; the FFI stays in the `rocksdb` crate. `eld-tendermint unsafe-reset-all` is the command that deletes chain data. Its tests are in `crates/node/tests/status.rs`.

`tests/vectors/` holds the hex copied from the Go commit in `proto/GO_REF`. `scripts/refresh-vectors.sh --check` fails when those files and that commit disagree. Run `scripts/refresh-vectors.sh` after the pin changes, then commit the new files.

## Dependency updates

Update Cargo crates and GitHub Actions by hand. `./scripts/ci.sh` has to pass, including `cargo audit` and `cargo deny`. Read the lockfile diff. A change in cryptography, encoding, or the database crate needs the same care as any other change to the wire format.

These versions are pinned. Change them by hand:

- `cargo-audit` and `cargo-deny` in `.github/workflows/ci.yml`. The workflow checks each download against a SHA-256. Change the version and the hash together.
- `gitleaks` in that same file.
- Rust 1.86.0. Change `rust-toolchain.toml` and `dtolnay/rust-toolchain@1.86.0` in the workflow together.

## Pull requests

The pull request template asks for three things:

- The Go reference commit you checked against.
- The tests you ran.
- Whether any encoding changed relative to the Go node.
