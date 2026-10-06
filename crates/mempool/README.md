# eld-tendermint-mempool

Tendermint 0.34 v0 mempool. Transactions are checked in arrival order, held until a block needs them, and checked again before they are kept.

Matches `mempool/v0` in the Go tree: FIFO `CheckTx`, reap in arrival order, and recheck. Gossip sends one transaction per message on channel `0x30` and does not send that transaction back to the peer that delivered it. This crate does not write a mempool write-ahead log. A `mempool.version` other than `v0` is rejected.

## Public API

`Mempool` runs `CheckTx` on each transaction, reaps transactions in the order they arrived, and runs `CheckTx` again when the pool rechecks. `PreCheck` and `PostCheck` are optional filters around `CheckTx`. `App` is the trait this crate calls to send `CheckTx` to the ABCI app. `Reactor` sends one transaction per message to peers on `MEMPOOL_CHANNEL` (`0x30`).

The v1 prioritized mempool is not implemented.
