# eld-tendermint-rs

[![CI](https://github.com/eldnetwork/eld-tendermint-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/eldnetwork/eld-tendermint-rs/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/eldnetwork/eld-tendermint-rs?include_prereleases&sort=date&display_name=tag)](https://github.com/eldnetwork/eld-tendermint-rs/releases/latest)
[![Docker image](https://img.shields.io/badge/docker-ghcr.io%2Feldnetwork%2Feld--tendermint--rs-2496ED?logo=docker&logoColor=white)](https://github.com/eldnetwork/eld-tendermint-rs/pkgs/container/eld-tendermint-rs)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.86.0-DEA584?logo=rust&logoColor=white)](rust-toolchain.toml)

## Project intro

eld-tendermint-rs is a Rust port of Tendermint 0.34.42.


Protocol specs are in `spec/`.

This port will be used as the base for later Eld network work to extend and modify the protocol for Eld's requirements, but those modifications will happen in a separate project. This project will remain a direct port.

## Quickstart

Check out the latest release tag.

Add tendermint configuration files to your Home directory. The Home directory can be any directory of your choice; pass it as `--home`.


Required config files:

```text
$HOME/.eld-tendermint/
├── config/
│   ├── addrbook.json
│   ├── config.toml
│   ├── genesis.json
│   ├── node_key.json
│   └── priv_validator_key.json
└── data/
    └── priv_validator_state.json
```

For a fresh start, delete every other file in `data/`except priv_validator_state.json

```bash
cargo run -p eld-tendermint-node -- start --home $HOME/.eld-tendermint
```
You will see a message that tendermint can't connect to the abci app...

Now start your abci app. Soon you should see block creation logs in the tendermint log.

## Docker

The published image is `ghcr.io/eldnetwork/eld-tendermint-rs`. The binary in that image is `eld-tendermint-rs`. Config, genesis, and validator keys are not in the image. Mount them at `TMHOME` (default `/tendermint-rs/.tendermint`).

```bash
docker build -t ghcr.io/eldnetwork/eld-tendermint-rs:local .
```

If `PROXY_APP` is set, the entrypoint passes it as `--proxy-app` for that process. Otherwise `proxy_app` comes from the mounted `config.toml`. This image does not include a kvstore app.

A tag `v0.34.24-eld-tm-rs.N` publishes the image after CI passes. Pushes run CI and do not publish.

## CI

Rust 1.86.0 (see `rust-toolchain.toml`). Install [gitleaks](https://github.com/gitleaks/gitleaks), [cargo-audit](https://github.com/rustsec/rustsec) 0.22.2+, and [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`brew install gitleaks cargo-audit cargo-deny` on macOS).

```bash
./scripts/ci.sh
```

That runs the same checks as GitHub Actions: the tendermint Go-version pin, `cargo fmt --check`, Clippy, build, test, `cargo audit`, `cargo deny`, and gitleaks. Rustc and Clippy warnings are treated as errors. A tag runs this workflow before the image job.

Dependabot opens pull requests for Cargo and GitHub Actions. It does not merge them. The policy is in [CONTRIBUTING.md](CONTRIBUTING.md). The SHA-256 pins for `cargo-audit` and `cargo-deny`, and the pinned `gitleaks` download, stay manual.

## What the project covers

- Consensus, with a write-ahead log and gossip to peers.
- ABCI 0.17 socket client.
- Block execution.
- RocksDB block store, with pruning below a retain height.
- P2p switch, with dial, accept, an address book, and PEX.
- v0 mempool.
- v0 fast sync.
- Evidence pool.
- Transaction index.
- File privval.
- Proto messages, Ed25519, and core types for blocks, votes, and evidence.
- `config.toml` and genesis loading.
- JSON-RPC `status`, `health`, `genesis`, `validators`, `blockchain`, `net_info`, `consensus_state`, `broadcast_tx_sync`, `broadcast_tx_async`, `broadcast_tx_commit`, `abci_query`, `block`, `commit`, `tx`, and `tx_search`.
- WebSocket `subscribe` and `unsubscribe` for `NewBlock` and `Tx`.

Changes so far are listed in [CHANGELOG.md](CHANGELOG.md).

## Crates

Every package is `0.0.1` and unpublished.

| Package | What it matches in the Go tree |
| --- | --- |
| `eld-tendermint-proto` | Prost messages generated from `proto/`. `tests/vectors.rs` checks them against the Go hex vectors. |
| `eld-tendermint-crypto` | `crypto/tmhash`, RFC-6962 Merkle roots and inclusion proofs, Ed25519 key bytes, Amino JSON, sign and verify. |
| `eld-tendermint-types` | Headers, blocks, votes, proposals, validators and proposer priority, `VoteSet` (+2/3), duplicate-vote evidence, genesis, `BitArray`, `PartSet`, `ValidateBasic`, and canonical sign bytes. |
| `eld-tendermint-config` | `config.toml` and `genesis.json`. The `eld-tendermint-config` binary prints chain id, moniker, proxy app, and the genesis validator set. |
| `eld-tendermint-privval` | `privval.FilePV`: load a Go validator key and sign a vote or proposal, replaying the same height, round, and step and rejecting conflicting bytes. A set `priv_validator_laddr` dials that signer instead of the local key. |
| `eld-tendermint-p2p` | `p2p.NodeKey`, the secret-connection handshake, the switch, TCP dial and accept, `addrbook.json`, and PEX on channel `0x00`. |
| `eld-tendermint-abci` | ABCI 0.17 socket client: `echo`, `info`, `check_tx`, `deliver_tx`, `commit`, `query`, `begin_block`, `end_block`, `init_chain`. The ignored live test is documented in `crates/abci/README.md`. |
| `eld-tendermint-store` | `store.BlockStore`. Keys are `H:`, `P:`, `C:`, `SC:`, `BH:`, and `blockStore`. `prune_blocks` deletes heights below a retain target after saving the new base. RocksDB on disk, an in-memory map in tests. A Go goleveldb directory is refused. |
| `eld-tendermint-state` | `MakeGenesisState`, `InitChain`, and `ApplyBlock`. A missing `stateKey` calls `InitChain` once. A validator update lands in the next set and becomes current one block later. State reloads from `stateKey`. DeliverTx results are indexed under the Go `tx.height` keys. |
| `eld-tendermint-evidence` | Evidence pool. A duplicate vote is verified against the current or last validator set. A light-client attack is verified against the trusted and common validator sets. Both are stored in `evidence.db`, gossiped on channel `0x38`, and included in one proposal. |
| `eld-tendermint-mempool` | v0 FIFO mempool. `CheckTx`, reap in arrival order, and recheck. Gossip sends one tx per message on channel `0x30` and does not send that tx back to the peer that sent it. |
| `eld-tendermint-blockchain` | v0 fast sync on channel `0x40`. A taller peer is asked for up to 20 blocks. Each block is applied at the next height after its commit matches the previous block. v1 and v2 are not started. |
| `eld-tendermint-consensus` | In-process rounds, with gossip on channels `0x20`–`0x23`. One and four validators commit height 1, and a locked validator re-proposes that block. A node one or two blocks behind catches up on `0x21` and `0x22` with fast sync off. The WAL is CRC32C-framed. The head stays `cs.wal/wal` and rotates to `wal.NNN` at 10 MiB. A prevote written before rotation is replayed from the older segment without a second signature. |
| `eld-tendermint-node` | `eld-tendermint start`. Loads one home (config, keys, RocksDB, ABCI `Info`, `InitChain` on a fresh home, mempool, consensus, evidence, PEX, and v0 fast sync when `fast_sync` is on). A set `priv_validator_laddr` dials that signer and does not read `priv_validator_key.json`. A refused dial exits before RPC starts. A commit `retain_height` prunes block-store heights below that target. Serves JSON-RPC `status`, `health`, `genesis`, `validators`, `blockchain`, `net_info`, `consensus_state`, `broadcast_tx_sync`, `broadcast_tx_async`, `broadcast_tx_commit`, `abci_query`, `block`, `commit`, `tx`, and `tx_search`. `GET /websocket` serves `subscribe` and `unsubscribe` for `NewBlock` and `Tx`. `eld-tendermint unsafe-reset-all` deletes `data/` and the address book, keeps config and keys, and writes a height-0 `priv_validator_state.json`. The next start calls `InitChain` once. |

## Database

Tendermint 0.34 defaults to goleveldb. goleveldb is a pure-Go port of Google LevelDB. It is the Tendermint 0.34 default because it has no C dependency. RocksDB is Facebook's fork of LevelDB. This Rust port uses RocksDB.

Both are embedded, single-process key-value stores built on a log-structured merge tree. You write batches, read by key, and scan ranges. Compaction runs in the background.

- NOTE: The tm go and tm rust files are not interchangeable. A Go data/ directory created with db_backend = "goleveldb" will not open under RocksDB.
- Key prefixes match the Go block store (H:, P:, C:, SC:, BH:, blockStore). The directory does not.
- Tests use the in-memory Db. The node writes RocksDB files in its own directory.
- blockstore, state, evidence, and tx_index stay separate databases, as in the Go node. `eld-tendermint start` opens blockstore, state, `evidence.db`, and `tx_index.db` unless `[tx_index] indexer` is `null`.
- `eld-tendermint unsafe-reset-all` deletes `data/` and the address book, and keeps `config.toml`, genesis, and the node and validator keys. `--keep-addr-book` leaves the address book. The reset home itself can be started by either binary. The `data/` files the next Rust `start` creates are RocksDB and cannot be opened by the Go node.

`tools/proto-compiler` is the prost-build binary used by `scripts/gen-proto.sh`.

```bash
cargo test --workspace
eld-tendermint start --home /path/to/node
eld-tendermint unsafe-reset-all --home /path/to/node
eld-tendermint-config --home /path/to/node
```

`cargo test --workspace` builds RocksDB for `eld-tendermint-store`. That build needs CMake. If CMake is not installed, install it with Homebrew. CMake 4.4.3 is enough:

```bash
brew install cmake
```

When `--home` is omitted, the node uses `TMHOME` if that variable is set, and otherwise `$HOME/.tendermint`, the same default as the Go binary.

## Proto

`proto/` is the schema. `crates/proto/` is the Rust library generated from it.

`proto/` is the Tendermint 0.34.42 schema, recorded in `proto/GO_REF`. It holds the 24 `.proto` files under `proto/tendermint/` plus `proto/third_party/gogoproto/gogo.proto`. These files are the source of truth for field numbers and message layout (ABCI 0.17.0). Cargo does not compile this directory. `scripts/gen-proto.sh` reads it and writes Prost output.

`crates/proto/` is the Cargo package `eld-tendermint-proto`. `src/prost/*.rs` is the generated output, and `src/lib.rs` exposes it as `eld_tendermint_proto::abci`, `::types`, and the other packages. Later crates depend on this package.

Edit `proto/tendermint/**` only when the Go schema changes, then regenerate. Do not hand-edit `crates/proto/src/prost/`.

`tests/vectors.rs` matches mempool txs, blockchain messages including `BlockResponse`, privval ping, pubkey, vote, and proposal rows, consensus messages, pex, connection packets, statesync, evidence, deliver-tx nil-versus-empty `Data`, and one `RequestEcho` round-trip. `TestVoteSignBytesTestVectors` lives in `crates/types/tests/sign_bytes.rs`. `TestDeriveSecretsAndChallengeGolden` is checked in `crates/p2p`. The length-prefixed `RequestEcho` frame is checked in `crates/abci`. `types/protobuf_test.go` generates keys and has no static hex.

Amino JSON for keys, the privval files, and `node_key.json` is implemented on the hand-written types. Prost ignores `jsontag` and `customname`, so the generated messages do not carry those names.

`tendermint-rs` (`tendermint-proto` 0.40) already has Prost types for this ABCI shape, under `tendermint_proto::v0_34`. This repo does not depend on that crate:

- Its `v0_34` module was generated from CometBFT `v0.34.35`, which is not Tendermint 0.34.42.
- The crate root re-exports CometBFT 0.38 (`pub use v0_38::*`), so `tendermint_proto::abci` is `FinalizeBlock`-era ABCI.
- `tendermint-abci` in that repo speaks the 0.38 socket codec.
- Those bindings are not tested against Eld’s Go hex vectors.

We copied the generator settings from `tendermint-rs`, not the library. The generator uses Prost 0.13. ABCI byte fields are the `bytes` type. `Timestamp` and `Duration` use Prost's own types instead of being generated from the schema.

## Spec

`spec/` is the Tendermint 0.34.42 spec. The markdown and the files it links to are left as they are. Do not edit them here.

## Still to do

- JSON-RPC methods other than the ones already served. A call to one of those returns `-32601`.
- v1 and v2 fast sync. The node starts fast sync only when the version is `v0`.
