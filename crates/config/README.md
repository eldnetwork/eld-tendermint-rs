# eld-tendermint-config

Loader for a Tendermint 0.34 `config.toml` and `genesis.json`.

Matches `config.Config` in the Go tree. Parsing starts from the Go defaults and overlays the keys present in the file. A key missing from the file keeps the Go default. `chain_id` is read from `genesis.json`.

## Public API

`load_home` reads `config.toml` and `genesis.json` from one node directory. `resolve_home` chooses the node directory from `--home`, then from the `TMHOME` environment variable, then from `$HOME/.tendermint`. `Config` holds the `base`, `rpc`, `p2p`, `mempool`, `statesync`, `fastsync`, `consensus`, `storage`, `tx_index`, and `instrumentation` sections. `genesis_file`, `db_dir`, `wal_file`, and the key-file helpers turn each path in `config.toml` into a path under the node directory, the same way Go's `filepath.Join` does.

The `eld-tendermint-config` binary takes `--home` and prints the chain id, the moniker, the proxy app, and the genesis validator set.

The `statesync` section of `config.toml` is parsed into `StateSyncConfig`. This crate does not download state snapshots.
