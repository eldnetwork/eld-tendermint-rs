# eld-tendermint-node

This crate is the `eld-tendermint` binary, the program you run. `eld-tendermint start` loads one node home and, in that same process, runs consensus, the mempool, the evidence pool, peer exchange, the content reactor, and v0 fast sync, dials the ABCI app, and serves REST and JSON-RPC.

Matches `node.Node` and `cmd/tendermint` in the Go tree for the `start` and `unsafe-reset-all` commands. Other crates do not call into this binary.

## Public API

`eld-tendermint start [--home <dir>] [--proxy-app <addr>]` keeps running and accepts REST and JSON-RPC on one listener until the process stops. The HTTP method chooses the API: `GET` is REST, `POST` is JSON-RPC. `OPTIONS` returns 200 with an empty body before either of those. Every REST and JSON-RPC response allows `https://explorer.eld.network` to call with `GET`, `POST`, and `OPTIONS`, and to send `Content-Type`.

`eld-tendermint unsafe-reset-all [--home <dir>] [--keep-addr-book]` deletes `data/` (or the configured `db_dir`) and `config/addrbook.json`, and writes a height-0 `priv_validator_state.json`. `config.toml`, `genesis.json`, and the node and validator keys stay.

`POST /` serves `health`, `status`, `genesis`, `validators`, `blockchain`, `net_info`, `consensus_state`, `broadcast_tx_sync`, `broadcast_tx_async`, `broadcast_tx_commit`, `abci_query`, `abci_info`, `block`, `commit`, `summary`, `tx`, `tx_search`, `content_push`, `content_request`, and `content_challenge`. `GET /status` returns that same `status` result, with JSON-RPC `id` `-1`. `summary` returns the current summary header and does not include the snapshot body. Any other JSON-RPC method name returns `-32601`. `GET /websocket` serves `subscribe` and `unsubscribe` for the queries `tm.event='NewBlock'` and `tm.event='Tx'`.

When `priv_validator_laddr` is set, `start` dials the address in `priv_validator_laddr` for signatures and does not read `priv_validator_key.json`. If that dial is refused, `start` exits before the listener starts. When ABCI `Commit` returns `retain_height`, block-store heights below `retain_height` are deleted.

v1 and v2 fast sync are not started. Calls to `block_by_hash`, `block_results`, `genesis_chunked`, `consensus_params`, `unconfirmed_txs`, `num_unconfirmed_txs`, `check_tx`, and `broadcast_evidence` return `-32601`.
