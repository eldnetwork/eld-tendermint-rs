//! Tendermint 0.34 `store.BlockStore`.
//!
//! Keys match the Go block store: `H:`, `P:`, `C:`, `SC:`, `BH:`, and `blockStore`.
//! The on-disk backend is RocksDB. A directory that already holds a Go goleveldb
//! layout (`CURRENT`, `LOG`, and `*.ldb`, with no RocksDB `IDENTITY` file) is refused.
//!
//! Deserialization of a present key is on-disk corruption and panics, with the key
//! in the message. Database get and set failures inside [`BlockStore`] panic the
//! same way. `save_block` returns [`Error`] for an incomplete part set or a
//! non-contiguous height. `prune_blocks` deletes older heights after the new base
//! is saved.

mod block_store;
mod db;
mod ensured;
mod error;
mod keys;

pub use block_store::{BlockMeta, BlockStore};
pub use db::{Batch, Db, MemDb, PrefixRow, RocksDb};
pub use error::Error;
