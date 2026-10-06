# eld-tendermint-store

Tendermint 0.34 block store. Blocks, parts, and commits are saved under the same key prefixes the Go node uses.

Matches `store.BlockStore` in the Go tree, and the get, set, and batch calls `BlockStore` makes on the database. The key prefixes are `H:`, `P:`, `C:`, `SC:`, `BH:`, and `blockStore`. The on-disk database is RocksDB. `RocksDb::open` refuses a Go goleveldb directory that contains `*.ldb` files, or `CURRENT` and `LOG` and no RocksDB `IDENTITY` file, and does not create RocksDB files in the goleveldb directory.

## Public API

`BlockStore` saves and loads blocks, block parts, commits, and block meta under the `H:`, `P:`, `C:`, `SC:`, `BH:`, and `blockStore` keys. `prune_blocks` deletes heights below a retain height after the new base height is saved. `RocksDb` is the on-disk `Db`. `MemDb` is an in-memory `Db` for tests. `Batch` queues `set` and `delete` operations and writes them in order.

A key that is present in the database but does not decode is treated as a corrupt store. `save_block` returns an error when the part set is incomplete, or when the block height is not one above the height already stored.
