# eld-tendermint-types

Tendermint 0.34 core types: headers, votes, validators, genesis documents, and blocks.

Matches `types` in the Go tree for the structs, `ValidateBasic`, header hashes, Merkle hashes, length-prefixed canonical sign bytes, and proposer-priority rotation. `eld-tendermint-state` executes transactions. This crate does not execute transactions.

## Public API

`Block`, `Header`, `Commit`, `Vote`, `Proposal`, `ValidatorSet`, `VoteSet`, `PartSet`, `BitArray`, `GenesisDoc`, and `Evidence` (`DuplicateVoteEvidence`, `LightClientAttackEvidence`, `EvidenceList`) are the main types. `BLOCK_PROTOCOL` is 11. `Time`, `Tx`, `ConsensusParams`, and `LightBlock` are also in this crate.

`VoteSet` records votes until the voted power is more than two thirds of the validator set. `ValidatorSet` increments proposer priority and selects the next proposer. `Vote` builds the length-prefixed canonical bytes for a vote. `Proposal` builds the length-prefixed canonical bytes for a proposal.
