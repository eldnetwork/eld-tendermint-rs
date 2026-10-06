# eld-tendermint-evidence

Tendermint 0.34 evidence pool. Duplicate votes and light-client attacks are checked, stored, and sent to peers.

Matches the Go evidence pool and its gossip on channel `0x38`. A duplicate vote is checked against the current validator set or the previous one, stored in `evidence.db`, and copied into one proposal. A light-client attack is checked against the trusted validator set and the common validator set. `Pool::add_light` stores a light-client attack when the caller passes the trusted header.

## Public API

`Pool` checks a duplicate vote or a light-client attack and writes the accepted evidence into `evidence.db`. `ProposalEvidence` is the stored evidence a proposer copies into the next block. `Reactor` sends evidence messages to peers on `EVIDENCE_CHANNEL` (`0x38`).

A light-client attack that arrives from a peer is not written to `evidence.db`. The pool has no block store, so the caller of `Pool::add_light` has to pass the trusted header. Stored evidence is not expired or pruned.
