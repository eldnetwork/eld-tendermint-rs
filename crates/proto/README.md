# eld-tendermint-proto

Prost messages generated from the Tendermint protos copied into `proto/`. Field numbers and message layout match Eld Tendermint `v0.34.24-eld.3` (ABCI 0.17.0).

Matches the generated Go types under `proto/tendermint`. This crate only holds the messages. It does not open an ABCI socket and does not start a node. The hex checks against the Go tests are in `tests/vectors.rs`. How to refresh the hex and re-run the checks is in the root README under "Verify compatibility".

Regenerate the messages with `scripts/gen-proto.sh`. Do not hand-edit `src/prost/`.

## Public API

`src/lib.rs` exposes one module per proto package: `abci`, `blockchain`, `consensus`, `crypto`, `libs`, `mempool`, `p2p`, `privval`, `rpc`, `state`, `statesync`, `store`, `types`, and `version`. Callers encode and decode the Prost messages exported from `src/lib.rs`. Amino JSON names from the Go `jsontag` options are not generated on the Prost messages, because Prost ignores `jsontag` and `customname`.
