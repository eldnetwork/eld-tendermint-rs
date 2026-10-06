//! `store.BlockStore`.
//!
//! The mutex guards `base` and `height` only. Rows are not locked, matching Go:
//! the database enforces its own concurrency, and the key encoding is not
//! lexicographic (`H:10` sorts before `H:2`).

use std::sync::Mutex;

use eld_tendermint_types::{Block, BlockId, Commit, Header, Part, PartSet};
use prost::Message;

use crate::db::{Batch, Db};
use crate::ensured::Ensured;
use crate::error::Error;
use crate::keys::{
    BLOCK_STORE_KEY, block_commit_key, block_hash_key, block_meta_key, block_part_key,
    seen_commit_key,
};

/// `types.BlockMeta`. Stored as prost `tendermint.types.BlockMeta`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMeta {
    pub block_id: BlockId,
    pub block_size: i64,
    pub header: Header,
    pub num_txs: i64,
}

struct StoreState {
    base: i64,
    height: i64,
}

/// Contiguous blocks from `base` through `height`, inclusive.
///
/// A present value that fails to decode is on-disk corruption. Those loads panic,
/// and the message includes the key. Go panics the same way. Database get and set
/// failures inside this type also panic.
pub struct BlockStore<D: Db> {
    db: D,
    state: Mutex<StoreState>,
}

impl<D: Db> BlockStore<D> {
    /// `NewBlockStore`. An empty database has base 0 and height 0.
    ///
    /// A stored state with `height > 0` and `base == 0` loads as base 1.
    ///
    /// # Panics
    ///
    /// Panics when `blockStore` is present and is not a `BlockStoreState`, or when
    /// the database read fails. The panic text includes `blockStore`.
    #[must_use]
    pub fn new(db: D) -> Self {
        let (base, height) = load_state(&db);
        Self {
            db,
            state: Mutex::new(StoreState { base, height }),
        }
    }

    /// First contiguous height, or 0 when the store is empty.
    #[must_use]
    pub fn base(&self) -> i64 {
        self.state.lock().ensured("block store state lock").base
    }

    /// Last contiguous height, or 0 when the store is empty.
    #[must_use]
    pub fn height(&self) -> i64 {
        self.state.lock().ensured("block store state lock").height
    }

    /// `height - base + 1`, or 0 when `height` is 0.
    #[must_use]
    pub fn size(&self) -> i64 {
        let state = self.state.lock().ensured("block store state lock");
        if state.height == 0 {
            0
        } else {
            state.height - state.base + 1
        }
    }

    /// `SaveBlock`.
    ///
    /// Parts are written before the block meta. The meta is the signal that the
    /// block exists. A nil `last_commit` does not write `C:{height-1}`, so a
    /// height-1 block with no last commit does not write `C:0`. A present last
    /// commit is written at `C:{height-1}` even when that key is `C:0`.
    ///
    /// An empty store (`base == 0`) accepts any first height. After that, the
    /// next block must be `height + 1`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NonContiguous`] when the height is not the next one, or
    /// [`Error::IncompletePartSet`] when `part_set` is not complete. Go panics
    /// for both. The contiguous check runs first, matching `SaveBlock`.
    ///
    /// # Panics
    ///
    /// Panics when a database write fails.
    pub fn save_block(
        &self,
        block: &Block,
        part_set: &PartSet,
        seen_commit: &Commit,
    ) -> Result<(), Error> {
        let height = block.header.height;
        let base = self.base();
        let wanted = self.height() + 1;
        if base > 0 && height != wanted {
            return Err(Error::NonContiguous {
                wanted,
                got: height,
            });
        }
        if !part_set.is_complete() {
            return Err(Error::IncompletePartSet);
        }

        for index in 0..part_set.total() {
            let part = part_set
                .get_part(index)
                .unwrap_or_else(|| panic!("complete part set is missing part {index}"));
            let bytes = part.to_proto().encode_to_vec();
            must_set(&self.db, &block_part_key(height, index), &bytes);
        }

        let hash = block
            .hash()
            .map(|hash| hash.as_bytes().to_vec())
            .unwrap_or_default();
        let block_id = BlockId {
            hash: hash.clone(),
            part_set_header: part_set.header(),
        };
        let meta = eld_tendermint_proto::types::BlockMeta {
            block_id: Some(block_id.to_proto()),
            block_size: i64::try_from(block.to_proto().encoded_len()).ensured("block size fits"),
            header: Some(block.header.to_proto()),
            num_txs: i64::try_from(block.data.as_slice().len()).ensured("tx count fits"),
        };
        must_set(&self.db, &block_meta_key(height), &meta.encode_to_vec());
        must_set(
            &self.db,
            &block_hash_key(&hash),
            format!("{height}").as_bytes(),
        );

        if let Some(commit) = &block.last_commit {
            must_set(
                &self.db,
                &block_commit_key(height - 1),
                &commit.to_proto().encode_to_vec(),
            );
        }

        must_set(
            &self.db,
            &seen_commit_key(height),
            &seen_commit.to_proto().encode_to_vec(),
        );

        {
            let mut state = self.state.lock().ensured("block store state lock");
            state.height = height;
            if state.base == 0 {
                state.base = height;
            }
        }
        self.save_state();
        Ok(())
    }

    /// `PruneBlocks`. Deletes heights from `base` up to but not including `height`.
    ///
    /// The new base is written to `blockStore` before each delete batch is applied.
    /// Every 1000 pruned blocks is one batch. That flush records base as the height
    /// just queued. The final flush records `height`. The tip is not deleted.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PruneHeight`] when `height` is not positive,
    /// [`Error::BeyondLatest`] when `height` is past the tip, or [`Error::BelowBase`]
    /// when `height` is below the current base. Pruning to the current base deletes
    /// nothing.
    ///
    /// # Panics
    ///
    /// Panics when a database write fails.
    pub fn prune_blocks(&self, height: i64) -> Result<u64, Error> {
        if height <= 0 {
            return Err(Error::PruneHeight);
        }
        let (base, latest) = {
            let state = self.state.lock().ensured("block store state lock");
            (state.base, state.height)
        };
        if height > latest {
            return Err(Error::BeyondLatest { height, latest });
        }
        if height < base {
            return Err(Error::BelowBase { height, base });
        }

        let mut pruned = 0u64;
        let mut batch = Batch::new();
        for current in base..height {
            let Some(meta) = self.load_block_meta(current) else {
                continue;
            };
            batch.delete(&block_meta_key(current));
            batch.delete(&block_hash_key(&meta.block_id.hash));
            batch.delete(&block_commit_key(current));
            batch.delete(&seen_commit_key(current));
            let parts = meta.block_id.part_set_header.total;
            for index in 0..parts {
                batch.delete(&block_part_key(current, index));
            }
            pruned += 1;
            if pruned % 1000 == 0 && pruned > 0 {
                self.flush_prune(&batch, current);
                batch = Batch::new();
            }
        }
        self.flush_prune(&batch, height);
        Ok(pruned)
    }

    /// `LoadBlockMeta`. `None` when the key is missing.
    ///
    /// # Panics
    ///
    /// Panics when `H:{height}` is present and is not a valid block meta. The
    /// panic text includes that key. A block id hash that does not equal the
    /// header hash is the same corruption.
    #[must_use]
    pub fn load_block_meta(&self, height: i64) -> Option<BlockMeta> {
        let key = block_meta_key(height);
        let bytes = must_get(&self.db, &key)?;
        Some(decode_block_meta(&key, &bytes))
    }

    /// `LoadBlockPart`. `None` when the key is missing.
    ///
    /// # Panics
    ///
    /// Panics when `P:{height}:{index}` is present and is not a valid part.
    #[must_use]
    pub fn load_block_part(&self, height: i64, index: u32) -> Option<Part> {
        let key = block_part_key(height, index);
        let bytes = must_get(&self.db, &key)?;
        let proto =
            eld_tendermint_proto::types::Part::decode(bytes.as_slice()).unwrap_or_else(|err| {
                corrupt(&key, &format!("unmarshal to tmproto.Part failed: {err}"))
            });
        Some(
            Part::try_from_proto(Some(&proto))
                .unwrap_or_else(|err| corrupt(&key, &format!("Error reading block part: {err}"))),
        )
    }

    /// `LoadBlock`. Missing meta, or any missing part, is `None`.
    ///
    /// # Panics
    ///
    /// Panics when the parts are present but are not a valid block. The panic
    /// text includes `H:{height}`.
    #[must_use]
    pub fn load_block(&self, height: i64) -> Option<Block> {
        let meta = self.load_block_meta(height)?;
        let mut buf = Vec::new();
        for index in 0..meta.block_id.part_set_header.total {
            let part = self.load_block_part(height, index)?;
            buf.extend_from_slice(&part.bytes);
        }
        let key = block_meta_key(height);
        let proto = eld_tendermint_proto::types::Block::decode(buf.as_slice())
            .unwrap_or_else(|err| corrupt(&key, &format!("Error reading block: {err}")));
        Some(
            Block::try_from_proto(&proto)
                .unwrap_or_else(|err| corrupt(&key, &format!("error from proto block: {err}"))),
        )
    }

    /// `LoadBlockByHash`. `None` when `BH:{hex}` is missing.
    ///
    /// # Panics
    ///
    /// Panics when the value is not an ASCII decimal height. The panic text
    /// includes the hash key.
    #[must_use]
    pub fn load_block_by_hash(&self, hash: &[u8]) -> Option<Block> {
        let key = block_hash_key(hash);
        let bytes = must_get(&self.db, &key)?;
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|err| corrupt(&key, &format!("failed to extract height: {err}")));
        let height = text.parse::<i64>().unwrap_or_else(|err| {
            corrupt(
                &key,
                &format!("failed to extract height from {text}: {err}"),
            )
        });
        self.load_block(height)
    }

    /// `LoadBlockCommit`. This is `block.last_commit` stored at `C:{height}`
    /// by the block at `height + 1`.
    ///
    /// # Panics
    ///
    /// Panics when `C:{height}` is present and is not a valid commit.
    #[must_use]
    pub fn load_block_commit(&self, height: i64) -> Option<Commit> {
        decode_commit(&self.db, &block_commit_key(height))
    }

    /// `LoadSeenCommit`. The +2/3 precommits seen for this height.
    ///
    /// # Panics
    ///
    /// Panics when `SC:{height}` is present and is not a valid commit.
    #[must_use]
    pub fn load_seen_commit(&self, height: i64) -> Option<Commit> {
        decode_commit(&self.db, &seen_commit_key(height))
    }

    /// Writes `base` into `blockStore`, then applies `batch`.
    fn flush_prune(&self, batch: &Batch, base: i64) {
        {
            let mut state = self.state.lock().ensured("block store state lock");
            state.base = base;
        }
        self.save_state();
        if let Err(err) = self.db.write_sync(batch) {
            panic!("failed to prune up to height {base}: {err}");
        }
    }

    fn save_state(&self) {
        let (base, height) = {
            let state = self.state.lock().ensured("block store state lock");
            (state.base, state.height)
        };
        let proto = eld_tendermint_proto::store::BlockStoreState { base, height };
        must_set_sync(&self.db, BLOCK_STORE_KEY, &proto.encode_to_vec());
    }
}

fn load_state<D: Db>(db: &D) -> (i64, i64) {
    let Some(bytes) = must_get(db, BLOCK_STORE_KEY) else {
        return (0, 0);
    };
    let proto = eld_tendermint_proto::store::BlockStoreState::decode(bytes.as_slice())
        .unwrap_or_else(|err| {
            corrupt(
                BLOCK_STORE_KEY,
                &format!("Could not unmarshal bytes: {err}"),
            )
        });
    let mut base = proto.base;
    let height = proto.height;
    if height > 0 && base == 0 {
        base = 1;
    }
    (base, height)
}

fn decode_block_meta(key: &[u8], bytes: &[u8]) -> BlockMeta {
    let proto = eld_tendermint_proto::types::BlockMeta::decode(bytes)
        .unwrap_or_else(|err| corrupt(key, &format!("unmarshal to tmproto.BlockMeta: {err}")));
    let block_id = match &proto.block_id {
        Some(block_id) => BlockId::try_from_proto(block_id)
            .unwrap_or_else(|err| corrupt(key, &format!("error from proto blockMeta: {err}"))),
        None => BlockId::default(),
    };
    let header = match &proto.header {
        Some(header) => Header::try_from_proto(header)
            .unwrap_or_else(|err| corrupt(key, &format!("error from proto blockMeta: {err}"))),
        None => corrupt(key, "error from proto blockMeta: missing header"),
    };
    let expected = header
        .hash()
        .map(|hash| hash.as_bytes().to_vec())
        .unwrap_or_default();
    if block_id.hash != expected {
        corrupt(key, "expected BlockID#Hash and Header#Hash to be the same");
    }
    BlockMeta {
        block_id,
        block_size: proto.block_size,
        header,
        num_txs: proto.num_txs,
    }
}

fn decode_commit<D: Db>(db: &D, key: &[u8]) -> Option<Commit> {
    let bytes = must_get(db, key)?;
    let proto = eld_tendermint_proto::types::Commit::decode(bytes.as_slice())
        .unwrap_or_else(|err| corrupt(key, &format!("error reading block commit: {err}")));
    Some(
        Commit::try_from_proto(&proto)
            .unwrap_or_else(|err| corrupt(key, &format!("Error reading block commit: {err}"))),
    )
}

fn must_get<D: Db>(db: &D, key: &[u8]) -> Option<Vec<u8>> {
    match db.get(key) {
        Ok(value) => value,
        Err(err) => panic!("block store db get {}: {err}", key_text(key)),
    }
}

fn must_set<D: Db>(db: &D, key: &[u8], value: &[u8]) {
    if let Err(err) = db.set(key, value) {
        panic!("block store db set {}: {err}", key_text(key));
    }
}

fn must_set_sync<D: Db>(db: &D, key: &[u8], value: &[u8]) {
    if let Err(err) = db.set_sync(key, value) {
        panic!("block store db set sync {}: {err}", key_text(key));
    }
}

fn corrupt(key: &[u8], detail: &str) -> ! {
    panic!("corrupt block store value at {}: {detail}", key_text(key))
}

fn key_text(key: &[u8]) -> String {
    String::from_utf8_lossy(key).into_owned()
}
