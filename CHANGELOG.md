# Changelog

All notable changes to this node are recorded here. The crates stay unpublished `0.0.1` builds. Tagged releases are the `eld-tendermint` node.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Fixed

- JSON-RPC hashes and addresses that are Go `bytes.HexBytes` use uppercase hex: transaction `hash` (`broadcast_tx_*`, `tx`, `tx_search`, Tx events), block IDs, header hashes, validator and proposer addresses, and commit `validator_address`. They were standard base64 before.

### Added

- JSON-RPC `abci_info` asks the app for Info on the query connection, so `eld-cli chain abci-info` can read the app version, height, and app hash.
- `tests/vectors/` and `scripts/refresh-vectors.sh`, so CI fails when the hex fixtures and `proto/GO_REF` disagree.
- Property tests for Merkle roots, Amino JSON, vote sign bytes, and part-set hashes.
- Apache-2.0 license, notices, and GitHub community files.
- README badges, a compatibility matrix, the home layout, every CLI flag, and a short README for each crate.

### Removed

- Dependabot configuration for Cargo and GitHub Actions. Updates are manual.

### Changed

- The container binary is `eld-tendermint`, the same name as the local binary. The image is still `ghcr.io/eldnetwork/eld-tendermint-rs`. A published tag writes the image digest onto the GitHub Release.
- Operator logs, time, the secret-connection nonce, and the WAL frame are described in the README. Prometheus metrics from the Go node are not served.
- `status` is Tendermint 0.34 JSON, so a client can read the local validator address.
- Crate manifests inherit authors, license, edition, and repository from the workspace, and each description matches what that crate actually does.
- Clippy denies `.unwrap()`, `.expect()`, `dbg!`, and `todo!` outside tests.
- `proto/` and `spec/` name the Tendermint v0.34.24 commit they were copied from.

## [v0.34.24-eld-tm-rs.3] - 2026-10-05

### Changed

- The proposal, part, and vote path is logged. A height that stops shows whether the body never arrived, the part proof failed, or the precommit was nil.

## [v0.34.24-eld-tm-rs.2] - 2026-10-05

### Fixed

- A bad proof-of-lock round is rejected. A re-proposal sends `ProposalPOL`. A proof-of-lock for another round is ignored.
- A proposal part set that grows past `Block.MaxBytes` drops that peer. The block is not decoded.
- Vote gossip sends one vote in Tendermint's order: the last commit before a current prevote, nothing the peer already acknowledged, and a commit precommit across a gap of three.
- A +2/3 prevote majority is announced on every poll and answered with `VoteSetBits`. A second block id drops the peer.
- Each gossip sends one block part the peer is missing, chosen from the live bits, and does not send it again.
- A peer three heights behind still gets one stored block part. A height below the store base, or at this node's height, does not.

### Changed

- Proposal blocks use the protobuf data budget. Header time comes from the commit median.
- A validator that only received the proposal header still gets the block body, so the block can commit when the proposer is not linked to every node.
- A node one or two blocks behind is sent one missing part at a time, and tells its peers when it switches to the committed block.
- The Tendermint spec is copied into `spec/`.

### Compatibility

- Votes, parts, valid blocks, and round age are sent the way a Go v0.34.24 peer expects.

## [v0.34.24-eld-tm-rs.1] - 2026-10-04

### Added

- Operator lines for ABCI, round steps, peers, and commits go to stderr. They use the Go messages and fields (`INFO` and `ERROR`). They are not a byte copy of the Go logger. Dial, proposal, vote, WAL, and shutdown failures use those same lines.
- `eld-tendermint unsafe-reset-all` deletes `data/` and the address book and writes a height-0 privval state. Config and keys stay. `--keep-addr-book` leaves the address book. The next `start` calls `InitChain` once.
- Remote privval dial. A set `priv_validator_laddr` signs through that socket and does not read `priv_validator_key.json`. A refused dial stops startup. A different signature for the same height, round, and step is a double-sign error. FilePV stays the default.
- Block pruning. `prune_blocks` deletes block-store rows below a retain height after the new base is saved. A commit `retain_height` prunes to that height. State and the tx index are left in place.
- Light-client attack evidence. An equivocation fixture verifies against the trusted and common validator sets, changes the block evidence hash, and is left out of the next proposal.
- Consensus WAL rotation. The head stays `cs.wal/wal`. At 10 MiB it is renamed to `wal.NNN` and replaced by an empty file. A prevote in an older segment is replayed without a second signature.
- RPC byte fields are standard base64, and Ed25519 public keys use the `tendermint/PubKeyEd25519` Amino envelope.
- JSON-RPC `genesis`, `validators`, `blockchain`, `net_info`, and `consensus_state`. `broadcast_tx_async` returns the CheckTx code and tx hash without waiting for the next block.
- Consensus catchup. A peer one or two heights behind is sent that block's parts on `0x21` and its seen-commit precommits on `0x22`, and commits the block once the votes are +2/3 and the block applies. A wider gap is left to fast sync.
- WebSocket `GET /websocket` serves `subscribe` and `unsubscribe` for `NewBlock` and `Tx`. A committed block is pushed as the commit result, and each DeliverTx is pushed as `ResultTx`.
- Transaction index in `tx_index.db`. JSON-RPC `tx` and `tx_search` return a committed DeliverTx after a restart. The queries served are `tx.height` and `tx.hash`.
- Evidence pool on channel `0x38`, stored in its own `evidence.db`. A duplicate vote is gossiped, included in the next proposed block once, and omitted after that block commits.
- v0 fast sync on channel `0x40`. A genesis node catches up to a taller peer when `fast_sync` is on, applying each block as soon as it is the next height.
- JSON-RPC `abci_query` returns the app's query value. `block` and `commit` return a saved block and the seen commit at the chain tip.
- JSON-RPC `broadcast_tx_sync` returns the CheckTx code and tx hash. `broadcast_tx_commit` returns the DeliverTx code after the next committed block, or a timeout error.
- A fresh home calls ABCI `InitChain` once and stores the app hash under `stateKey`. A restart calls `Info` only.
- `eld-tendermint start` loads one home and serves JSON-RPC `status` and `health`. An unknown method returns `-32601`.
- PEX and the dial loop. A persistent peer completes the secret handshake, and an address learned for another peer is dialed and reloaded from `addrbook.json`.
- Consensus gossip. Proposals, block parts, and votes move between switches on channels `0x20`–`0x23`, and four validators commit height 1 across the network.
- Mempool gossip. A transaction checked on one node is reaped on a peer, and it is not echoed back to the sender.
- P2P switch. Two peers reassemble a channel message on a secret connection. An unknown channel, an oversized payload, or a bad frame stops that peer.
- Consensus WAL. Records are CRC32C-framed `TimedWALMessage`s in one append-only file. A prevote that was `fsync`ed is replayed after reload without a second signature, and a conflicting vote at the same height, round, and step leaves the stored signature unchanged.
- In-process consensus. One and four validators commit height 1, and a validator that locked in round 0 re-proposes that block.
- v0 mempool. Transactions are checked, reaped in arrival order, and dropped on recheck.
- `ApplyBlock` and validator-set updates. A genesis block can rotate the next validator set, and that state reloads from `stateKey`.
- Block store. Blocks, parts, and commits persist under the Go key prefixes (`H:`, `P:`, `C:`, `SC:`, `BH:`, `blockStore`).
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
- Prost messages generated from the protos vendored in `proto/` (Tendermint v0.34.24, ABCI 0.17.0), plus the first mempool, blockchain, privval, and ABCI hex checks.
- A local CI script, run on every push, and required before a tag publishes the image.
- A GitHub Actions job publishes `ghcr.io/eldnetwork/eld-tendermint-rs` after that CI passes on a `v0.34.24-eld-tm-rs.N` tag.

### Compatibility

- RocksDB is the on-disk backend. A Go `data/` directory created with goleveldb is not opened. Key prefixes match the Go block store. The directory format does not.
- `unsafe-reset-all` leaves a home that either binary can start. The `data/` files the next Rust `start` creates are RocksDB and cannot be opened by the Go node.

[Unreleased]: https://github.com/eldnetwork/eld-tendermint-rs/compare/v0.34.24-eld-tm-rs.3...HEAD
[v0.34.24-eld-tm-rs.3]: https://github.com/eldnetwork/eld-tendermint-rs/compare/v0.34.24-eld-tm-rs.2...v0.34.24-eld-tm-rs.3
[v0.34.24-eld-tm-rs.2]: https://github.com/eldnetwork/eld-tendermint-rs/compare/v0.34.24-eld-tm-rs.1...v0.34.24-eld-tm-rs.2
[v0.34.24-eld-tm-rs.1]: https://github.com/eldnetwork/eld-tendermint-rs/releases/tag/v0.34.24-eld-tm-rs.1
