# eld-tendermint-rs

Rust port of the Tendermint consensus engine used by Eld. Encodings stay byte-compatible with the Go node at `v0.34.24-eld.3`.

The workspace covers proto messages, Ed25519, core types (blocks, evidence, and proposer priority), `config.toml` and genesis loading, file privval, the secret-connection handshake, and an ABCI 0.17 socket client. Consensus, the p2p reactor, mempool, WAL, and RPC come later. History of what has landed is in [CHANGELOG.md](CHANGELOG.md).

## Crates

Every package is `0.0.1` and unpublished.

| Package | What it matches in the Go tree |
| --- | --- |
| `eld-tendermint-proto` | Prost messages generated from `proto/`. `tests/vectors.rs` checks them against the Go hex vectors. |
| `eld-tendermint-crypto` | `crypto/tmhash`, RFC-6962 Merkle roots and inclusion proofs, Ed25519 key bytes, Amino JSON, sign and verify. |
| `eld-tendermint-types` | Headers, blocks, votes, proposals, validators and proposer priority, duplicate-vote evidence, genesis, `BitArray`, `PartSet`, `ValidateBasic`, and canonical sign bytes. |
| `eld-tendermint-config` | `config.toml` and `genesis.json`. The `eld-tendermint-config` binary prints chain id, moniker, proxy app, and the genesis validator set. |
| `eld-tendermint-privval` | `privval.FilePV`: load a Go validator key and sign a vote or proposal, replaying the same height, round, and step and rejecting conflicting bytes. |
| `eld-tendermint-p2p` | `p2p.NodeKey` and the secret-connection handshake. Loads `node_key.json` and checks `deriveSecrets` against the Go golden file. No reactor or dial loop. |
| `eld-tendermint-abci` | ABCI 0.17 socket client: `echo`, `info`, `check_tx`, `deliver_tx`, `commit`, `query`, `begin_block`, `end_block`, `init_chain`. The ignored live test is documented in `crates/abci/README.md`. |

`tools/proto-compiler` is the prost-build binary used by `scripts/gen-proto.sh`.

```bash
cargo test --workspace
eld-tendermint-config --home /path/to/node
```

`cargo test --workspace` builds RocksDB for `eld-tendermint-store`. That build needs CMake. If CMake is not installed, install it with Homebrew. CMake 4.4.3 is enough:

```bash
brew install cmake
```

`--home` defaults the same way as the Go binary when the flag is omitted.

## Proto

`proto/` is the schema. `crates/proto/` is the Rust library generated from it.

`proto/` is a copy of the Eld Go tree at `v0.34.24-eld.3` (`79dcdd712`), recorded in `proto/GO_REF`. It holds the 24 `.proto` files under `proto/tendermint/` plus `proto/third_party/gogoproto/gogo.proto`. These files are the source of truth for field numbers and message layout, and they match upstream Tendermint `v0.34.24` (ABCI 0.17.0). Cargo does not compile this directory. `scripts/gen-proto.sh` reads it and writes Prost output.

`crates/proto/` is the Cargo package `eld-tendermint-proto`. `src/prost/*.rs` is that generated output, and `src/lib.rs` exposes it as `eld_tendermint_proto::abci`, `::types`, and the other packages. Later crates depend on this package.

Edit `proto/tendermint/**` only when the Go schema changes, then regenerate. Do not hand-edit `crates/proto/src/prost/`.

`tests/vectors.rs` matches mempool txs, blockchain messages including `BlockResponse`, privval ping, pubkey, vote, and proposal rows, consensus messages, pex, connection packets, statesync, evidence, deliver-tx nil-versus-empty `Data`, and one `RequestEcho` round-trip. `TestVoteSignBytesTestVectors` lives in `crates/types/tests/sign_bytes.rs`. `TestDeriveSecretsAndChallengeGolden` is checked in `crates/p2p`. The length-prefixed `RequestEcho` frame is checked in `crates/abci`. `types/protobuf_test.go` generates keys and has no static hex.

Amino JSON for keys, the privval files, and `node_key.json` is implemented on the hand-written types. Prost ignores `jsontag` and `customname`, so the generated messages do not carry those names.

`tendermint-rs` (`tendermint-proto` 0.40) already has Prost types for this ABCI shape, under `tendermint_proto::v0_34`. This repo does not depend on that crate:

- Its `v0_34` module was generated from CometBFT `v0.34.35`, a later pin than Eld `v0.34.24-eld.3`.
- The crate root re-exports CometBFT 0.38 (`pub use v0_38::*`), so `tendermint_proto::abci` is `FinalizeBlock`-era ABCI.
- `tendermint-abci` in that repo speaks the 0.38 socket codec.
- Those bindings are not tested against Eld’s Go hex vectors.

`tendermint-rs` `tools/proto-compiler` is only the prost-build recipe: prost 0.13, `bytes` for ABCI fields, and extern paths for `Timestamp` and `Duration`.

## Still to do

- **Consensus.** The state machine is not ported. Blocks and votes can be built and checked. They are not executed.
- **p2p reactor.** `NodeKey` and the secret connection are in place. The switch, PEX, and dial loop are not.
- **Mempool.** Proto messages exist. The reactor does not.
- **WAL.** The consensus write-ahead log is not ported.
- **RPC.** The JSON-RPC server and client are not ported.
