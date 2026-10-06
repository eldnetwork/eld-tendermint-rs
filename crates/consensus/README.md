# eld-tendermint-consensus

Tendermint 0.34 consensus for validators in one process, plus gossip of proposals, block parts, and votes on the peer switch.

Matches the Go consensus state machine and `consensus` message gossip. Round steps, proposals, and votes move by method call inside one process. `Reactor` sends `tendermint.consensus.Message` on the switch. The write-ahead log is CRC32C-framed. The head file stays `cs.wal/wal` and is renamed to `wal.NNN` at 10 MiB. A prevote written before that rename is replayed from the older segment without a second signature.

This crate does not download blocks by fast sync and does not run state sync. A peer whose height is still inside this node's block store, and behind this node's consensus height, is sent the missing block parts on channel `0x21` and the seen-commit precommits on channel `0x22`. A gap wider than the blocks still stored is handled by `eld-tendermint-blockchain`.

## Public API

`Node` runs propose, prevote, and precommit for one validator. `Group` runs propose, prevote, and precommit for several validators in one process. `Step` is propose, prevote, or precommit. `Wal` appends each proposal and vote to `cs.wal/wal` and reads the appended proposals and votes back after a restart. `Reactor` sends round state on `STATE_CHANNEL` (`0x20`), block parts on `DATA_CHANNEL` (`0x21`), votes on `VOTE_CHANNEL` (`0x22`), and vote-set bits on `VOTE_SET_BITS_CHANNEL` (`0x23`).

A proposer that has an evidence pool copies pending duplicate votes into the block it proposes. In the tests, one validator commits height 1, and four validators commit height 1. A validator that locked a block in round 0 proposes that same block again in the next round.
