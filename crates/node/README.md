# eld-tendermint-node

This crate is the `eld-tendermint` binary, the program you run. `eld-tendermint start` loads one node home and, in that same process, runs consensus, the mempool, the evidence pool, peer exchange, and v0 fast sync, dials the ABCI app, and serves JSON-RPC.

Matches `node.Node` and `cmd/tendermint` in the Go tree for the `start` and `unsafe-reset-all` commands. Other crates do not call into this binary.

## Public API

`eld-tendermint start [--home <dir>] [--proxy-app <addr>]` keeps running and accepts JSON-RPC connections until the process stops.

`eld-tendermint unsafe-reset-all [--home <dir>] [--keep-addr-book]` deletes `data/` (or the configured `db_dir`) and `config/addrbook.json`, and writes a height-0 `priv_validator_state.json`. `config.toml`, `genesis.json`, and the node and validator keys stay.

`POST /` serves `health`, `status`, `genesis`, `validators`, `blockchain`, `net_info`, `consensus_state`, `broadcast_tx_sync`, `broadcast_tx_async`, `broadcast_tx_commit`, `abci_query`, `abci_info`, `block`, `commit`, `tx`, and `tx_search`. Any other JSON-RPC method name returns `-32601`. `GET /websocket` serves `subscribe` and `unsubscribe` for the queries `tm.event='NewBlock'` and `tm.event='Tx'`.

When `priv_validator_laddr` is set, `start` dials the address in `priv_validator_laddr` for signatures and does not read `priv_validator_key.json`. If that dial is refused, `start` exits before JSON-RPC listens. When ABCI `Commit` returns `retain_height`, block-store heights below `retain_height` are deleted.

v1 and v2 fast sync are not started. Calls to `block_by_hash`, `block_results`, `genesis_chunked`, `consensus_params`, `unconfirmed_txs`, `num_unconfirmed_txs`, `check_tx`, and `broadcast_evidence` return `-32601`.
