# eld-tendermint-state

Tendermint 0.34 chain state: build the genesis state, call `InitChain`, and apply a block.

Matches `state` in the Go tree for the genesis state record, block execution, and the `stateKey` database record. This crate does not keep a mempool, an evidence pool, or a block pruner.

## Public API

`make_genesis_state` builds the height-0 state record from `genesis.json`. `load_or_init_chain` loads the bytes stored at `stateKey`, or calls ABCI `InitChain` once when `stateKey` is missing. `apply_block` executes a block against the ABCI app. `validate_block` checks a block before `apply_block` runs. A validator-set update is stored as the next validator set and becomes the current validator set one block later. `StateStore` reads and writes the `stateKey` record. `TM_CORE_SEMVER` is the string `0.34.24`.

`TxIndex` stores each committed DeliverTx under the Go key `tx.height/{height}/{height}/{index}` and under the transaction hash. `CommitEvents` is the callback `apply_block` invokes so the node can publish a `NewBlock` event and a `Tx` event.
