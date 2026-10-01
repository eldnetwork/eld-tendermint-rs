# eld-tendermint-rs

Rust port of the Tendermint consensus engine used by Eld. Encodings must stay byte-compatible with the Go node.

## Proto

`proto/` is the schema. `crates/proto/` is the Rust library generated from it.

`proto/` is a copy of the Eld Go tree at `v0.34.24-eld.3` (`79dcdd712`), recorded in `proto/GO_REF`. It holds the 24 `.proto` files under `proto/tendermint/` plus `proto/third_party/gogoproto/gogo.proto`. These files are the source of truth for field numbers and message layout, and they match upstream Tendermint `v0.34.24` (ABCI 0.17.0). Cargo does not compile this directory. `scripts/gen-proto.sh` reads it and writes Prost output.

`crates/proto/` is the Cargo package `eld-tendermint-proto`. `src/prost/*.rs` is that generated output, and `src/lib.rs` exposes it as `eld_tendermint_proto::abci`, `::types`, and the other packages. `tests/vectors.rs` checks that encoding matches the Go hex vectors. Later crates depend on this package.

Edit `proto/tendermint/**` only when the Go schema changes, then regenerate. Do not hand-edit `crates/proto/src/prost/`.

`tendermint-rs` (`tendermint-proto` 0.40) already has Prost types for this ABCI shape, under `tendermint_proto::v0_34`. This repo does not depend on that crate:

- Its `v0_34` module was generated from CometBFT `v0.34.35`, a later pin than Eld `v0.34.24-eld.3`.
- The crate root re-exports CometBFT 0.38 (`pub use v0_38::*`), so `tendermint_proto::abci` is `FinalizeBlock`-era ABCI.
- `tendermint-abci` in that repo speaks the 0.38 socket codec.
- Those bindings are not tested against Eld’s Go hex vectors.

`tendermint-rs` `tools/proto-compiler` is only the prost-build recipe: prost 0.13, `bytes` for ABCI fields, and extern paths for `Timestamp` and `Duration`.

### Still to do

Protobuf field numbers and names are generated. These pieces of step 1 are not done yet:

- **Amino and gogoproto JSON tags are not on the Rust types.** Prost ignores `jsontag`, `customname`, and names like `tendermint/PubKeyEd25519`. The generated structs have no `serde` derive. Those tags matter for JSON, not for protobuf bytes. Add them with the types and RPC crates.
- **Wire compatibility covers the static Go hex vectors.** `tests/vectors.rs` matches mempool txs, blockchain messages including `BlockResponse`, privval ping/pubkey/vote/proposal rows, consensus messages, pex, connection packets, statesync, evidence, deliver-tx nil-versus-empty `Data`, and one `RequestEcho` round-trip. `TestVoteSignBytesTestVectors` lives in `crates/types/tests/sign_bytes.rs`. Still for a later crate: the secret-connection handshake and `TestDeriveSecretsAndChallengeGolden` (a KDF golden file, not a proto message). `types/protobuf_test.go` generates keys and has no static hex.
- **No live Go node check.** Nothing in this crate has encoded or decoded bytes against a running `eld-tendermint` process.
- **gRPC service stubs were not generated.** `ABCIApplication` and `BroadcastAPI` are messages only. Socket framing and gRPC servers come with the later `abci` crate.
