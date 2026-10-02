# Changelog

All notable changes to this workspace are recorded here. The packages are unpublished `0.0.1` builds, so everything below is unreleased.

## Unreleased

### Added

- `Block` and `MakeBlock`. Data, evidence, and last-commit hashes, `ValidateBasic`, and `MakePartSet` match the Go block tests, including the `Hello World` protobuf bytes.
- Proposer priority on `ValidatorSet`. Increment, copy, and the proposer sequence for fixed voting powers match the Go tests.
- `DuplicateVoteEvidence` and `EvidenceList`. A conflicting vote pair verifies against a validator set, and the block evidence hash changes when the list does.
- `NodeKey` and the secret-connection handshake. `node_key.json` loads the Amino Ed25519 key, `deriveSecrets` matches the Go golden file, and two in-process peers exchange one frame.
- ABCI 0.17 socket client. A framed `RequestEcho` matches the Go `WriteMessage` vector. `echo_and_info_against_node` stays ignored unless a node is dialed; see `crates/abci/README.md`.
- Proto hex coverage for the remaining static Go vectors: blockchain `BlockResponse`, privval vote and proposal rows, consensus messages, pex, connection packets, statesync, and duplicate-vote evidence. Encode and decode are both checked. Vote sign-byte vectors stay in `crates/types`.
- Ed25519 `sign` and `verify`, plus `FilePV`. A Go `priv_validator_key.json` loads, a replay of the same height, round, and step returns the stored signature, and a conflicting payload at that step is rejected. State is written with a temp file, `fsync`, and rename before the signature is returned.
- Merkle inclusion proofs (`Proof`, `proofs_from_byte_slices`, `verify`) and `Part` / `PartSet`, using the same 64 KiB part size and part-set hash as the Go node.
- `BitArray` with the Go JSON `x`/`_` string and the `libs.bits` proto shape, so vote sets and part sets can track bits the same way.
- `config.toml` and genesis loaders that start from the Go defaults and overlay the file. `eld-tendermint-config` prints chain id, moniker, proxy app, and the genesis validator set for a node home.
- Core types with `ValidateBasic`, header and Merkle hashes, and length-prefixed canonical sign bytes for votes and proposals.
- Ed25519 key bytes and Amino JSON (`tendermint/PubKeyEd25519`, `tendermint/PrivKeyEd25519`) so addresses and validator key files match the Go node.
- Prost messages generated from the Eld Tendermint `v0.34.24-eld.3` protos (ABCI 0.17.0), plus the first mempool, blockchain, privval, and ABCI hex checks.
