# eld-tendermint-blockchain

v0 fast sync for Tendermint 0.34. A node that is behind asks a taller peer for the missing blocks and applies them in height order.

Matches `blockchain/v0` in the Go tree. Each request on channel `0x40` asks for at most 20 blocks. A block is applied only when its height is the next height this node expects. The commit stored inside block H is checked against block H-1.

## Public API

`Reactor` sends `BlockRequest` messages to a taller peer, reads each `BlockResponse`, checks the commit against the previous block, and applies the block at the next height. `BLOCKCHAIN_CHANNEL` is `0x40`. Fast-sync block requests and block responses are sent on channel `0x40`. `channel_descriptors` returns channel `0x40` and its size limit so `eld-tendermint start` can register v0 fast sync on the peer switch.

v1 and v2 fast sync are not in this crate. `eld-tendermint start` constructs `Reactor` only when `fast_sync` is on and `fastsync.version` is `v0`.
