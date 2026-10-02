//! Cases from `store/store_test.go` that do not need pruning or a genesis file.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;

use eld_tendermint_proto::types::BlockIdFlag;
use eld_tendermint_store::{Batch, BlockStore, Db, Error, MemDb, RocksDb};
use eld_tendermint_types::{
    ADDRESS_SIZE, Block, BlockId, Commit, CommitSig, EvidenceList, PartSet, PartSetHeader, Time,
    Tx, Txs,
};
use prost::Message;

fn height0_commit() -> Commit {
    Commit {
        height: 0,
        round: 0,
        block_id: BlockId::default(),
        signatures: Vec::new(),
    }
}

fn seen_commit(height: i64) -> Commit {
    Commit {
        height,
        round: 0,
        block_id: BlockId {
            hash: vec![0xab; 32],
            part_set_header: PartSetHeader {
                total: 1,
                hash: vec![0xcd; 32],
            },
        },
        signatures: vec![CommitSig {
            block_id_flag: BlockIdFlag::Commit,
            validator_address: vec![0x11; ADDRESS_SIZE],
            timestamp: Time::GO_ZERO,
            signature: vec![1],
        }],
    }
}

fn test_block(height: i64, last_commit: Option<Commit>, tx: u8) -> (Block, PartSet) {
    let mut block = Block::make_block(
        height,
        Txs::new(vec![Tx::new(vec![tx])]),
        last_commit,
        EvidenceList::default(),
    );
    block.header.proposer_address = vec![0x22; ADDRESS_SIZE];
    block.header.validators_hash = vec![0x33; 32];
    let parts = block.make_part_set(8).expect("part set");
    assert!(parts.total() > 1, "part size 8 should split the block");
    (block, parts)
}

fn mem_store() -> (Arc<MemDb>, BlockStore<Arc<MemDb>>) {
    let db = Arc::new(MemDb::new());
    let store = BlockStore::new(Arc::clone(&db));
    (db, store)
}

#[test]
fn empty_store_is_zero() {
    let (_db, store) = mem_store();
    assert_eq!(store.base(), 0);
    assert_eq!(store.height(), 0);
    assert_eq!(store.size(), 0);
    assert!(store.load_block(1).is_none());
    assert!(store.load_block_meta(1).is_none());
    assert!(store.load_block_part(1, 0).is_none());
    assert!(store.load_block_commit(1).is_none());
    assert!(store.load_seen_commit(1).is_none());
    assert!(store.load_block_by_hash(&[0x33; 32]).is_none());
}

#[test]
fn save_two_blocks_reopens_with_the_same_hash_parts_and_commits() {
    let (db, store) = mem_store();
    let (block1, parts1) = test_block(1, Some(height0_commit()), 1);
    let seen1 = seen_commit(1);
    store
        .save_block(&block1, &parts1, &seen1)
        .expect("save height 1");
    assert_eq!(store.base(), 1);
    assert_eq!(store.height(), 1);
    assert_eq!(store.size(), 1);

    let (block2, parts2) = test_block(2, Some(seen1.clone()), 2);
    let seen2 = seen_commit(2);
    store
        .save_block(&block2, &parts2, &seen2)
        .expect("save height 2");

    drop(store);
    let store = BlockStore::new(db);

    assert_eq!(store.base(), 1);
    assert_eq!(store.height(), 2);
    assert_eq!(store.size(), 2);

    let loaded1 = store.load_block(1).expect("block 1");
    let loaded2 = store.load_block(2).expect("block 2");
    assert_eq!(loaded1, block1);
    assert_eq!(loaded2, block2);
    assert_eq!(loaded1.hash(), block1.hash());
    assert_eq!(loaded2.hash(), block2.hash());
    assert!(loaded1.hash().is_some());

    let meta1 = store.load_block_meta(1).expect("meta 1");
    let meta2 = store.load_block_meta(2).expect("meta 2");
    assert_eq!(meta1.block_id.part_set_header, parts1.header());
    assert_eq!(meta2.block_id.part_set_header, parts2.header());
    assert_eq!(
        meta1.block_id.hash,
        block1.hash().expect("hash").as_bytes().to_vec()
    );

    assert_eq!(
        store.load_block_commit(0).expect("commit at 0"),
        height0_commit()
    );
    assert_eq!(store.load_block_commit(1).expect("commit at 1"), seen1);
    assert_eq!(store.load_seen_commit(1).expect("seen 1"), seen1);
    assert_eq!(store.load_seen_commit(2).expect("seen 2"), seen2);

    let by_hash = store
        .load_block_by_hash(block1.hash().expect("hash").as_bytes())
        .expect("hash index");
    assert_eq!(by_hash, block1);
    assert!(store.load_block(3).is_none());
}

#[test]
fn saved_keys_use_go_prefixes() {
    let (db, store) = mem_store();
    let (block, parts) = test_block(1, Some(height0_commit()), 7);
    let seen = seen_commit(1);
    store.save_block(&block, &parts, &seen).expect("save");

    assert!(db.get(b"H:1").expect("get").is_some());
    assert!(db.get(b"P:1:0").expect("get").is_some());
    assert!(db.get(b"P:1:1").expect("get").is_some());
    assert!(db.get(b"SC:1").expect("get").is_some());
    assert!(db.get(b"C:0").expect("get").is_some());
    assert!(db.get(b"blockStore").expect("get").is_some());

    let hash = block.hash().expect("hash");
    let hash_key = format!("BH:{}", hex::encode(hash.as_bytes()));
    assert_eq!(
        db.get(hash_key.as_bytes()).expect("get").as_deref(),
        Some(b"1".as_slice())
    );
    assert!(
        hash_key
            .strip_prefix("BH:")
            .expect("prefix")
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
    );
}

#[test]
fn nil_last_commit_does_not_write_c0() {
    let (db, store) = mem_store();
    let (block, parts) = test_block(1, None, 9);
    store
        .save_block(&block, &parts, &seen_commit(1))
        .expect("save nil last commit");
    assert!(db.get(b"C:0").expect("get").is_none());
    assert_eq!(
        db.get(b"BH:").expect("get").as_deref(),
        Some(b"1".as_slice())
    );
}

#[test]
fn height_three_after_height_one_is_rejected() {
    let (_db, store) = mem_store();
    let (block1, parts1) = test_block(1, Some(height0_commit()), 1);
    store
        .save_block(&block1, &parts1, &seen_commit(1))
        .expect("save 1");
    let (block3, parts3) = test_block(3, Some(seen_commit(2)), 3);
    let err = store
        .save_block(&block3, &parts3, &seen_commit(3))
        .expect_err("not contiguous");
    let message = err.to_string();
    match err {
        Error::NonContiguous { wanted, got } => {
            assert_eq!(wanted, 2);
            assert_eq!(got, 3);
        }
        other => panic!("unexpected error: {other}"),
    }
    assert!(message.contains('2') && message.contains('3'), "{message}");
    assert_eq!(store.height(), 1);
}

#[test]
fn empty_store_accepts_a_first_block_at_height_five() {
    let (_db, store) = mem_store();
    let (block, parts) = test_block(5, Some(height0_commit()), 5);
    store
        .save_block(&block, &parts, &seen_commit(5))
        .expect("first block at 5");
    assert_eq!(store.base(), 5);
    assert_eq!(store.height(), 5);
    assert_eq!(store.size(), 1);
}

#[test]
fn incomplete_part_set_is_rejected() {
    let (_db, store) = mem_store();
    let (block, _parts) = test_block(1, Some(height0_commit()), 1);
    let incomplete = PartSet::from_header(PartSetHeader {
        total: 2,
        hash: vec![0xab; 32],
    });
    assert!(!incomplete.is_complete());
    let err = store
        .save_block(&block, &incomplete, &seen_commit(1))
        .expect_err("incomplete");
    assert!(matches!(err, Error::IncompletePartSet));
    assert_eq!(store.height(), 0);
}

#[test]
fn missing_part_makes_the_block_missing() {
    let (db, store) = mem_store();
    let (block, parts) = test_block(1, Some(height0_commit()), 1);
    store
        .save_block(&block, &parts, &seen_commit(1))
        .expect("save");
    db.delete(b"P:1:0").expect("delete part");
    assert!(store.load_block(1).is_none());
    assert!(store.load_block_meta(1).is_some());
}

#[test]
fn stored_state_without_base_loads_as_base_one() {
    let db = Arc::new(MemDb::new());
    let state = eld_tendermint_proto::store::BlockStoreState {
        base: 0,
        height: 1000,
    };
    db.set_sync(b"blockStore", &state.encode_to_vec())
        .expect("set state");
    let store = BlockStore::new(Arc::clone(&db));
    assert_eq!(store.base(), 1);
    assert_eq!(store.height(), 1000);
    assert_eq!(store.size(), 1000);

    db.set_sync(b"blockStore", &[]).expect("clear");
    let store = BlockStore::new(db);
    assert_eq!(store.base(), 0);
    assert_eq!(store.height(), 0);
}

#[test]
fn corrupt_meta_panics_with_the_key() {
    let db = Arc::new(MemDb::new());
    db.set(b"H:1", b"Tendermint-Meta").expect("set garbage");
    let store = BlockStore::new(Arc::clone(&db));
    let panicked = catch_unwind(AssertUnwindSafe(|| {
        let _ = store.load_block_meta(1);
    }));
    let payload = panicked.expect_err("corrupt meta panics");
    let text = panic_text(&payload);
    assert!(text.contains("H:1"), "{text}");
}

#[test]
fn memdb_batch_and_prefix_iter() {
    let db = MemDb::new();
    db.set(b"k2", b"gone").expect("set");
    db.set(b"P:2:0", b"c").expect("set");
    db.set(b"H:1", b"meta").expect("set");

    let mut batch = Batch::new();
    batch.set(b"P:1:1", b"b");
    batch.set(b"P:1:0", b"a");
    batch.delete(b"k2");
    db.write_sync(&batch).expect("write");

    assert_eq!(
        db.get(b"P:1:0").expect("get").as_deref(),
        Some(b"a".as_slice())
    );
    assert_eq!(db.get(b"k2").expect("get"), None);
    assert_eq!(
        db.iter_prefix(b"P:1:").expect("prefix"),
        vec![
            (b"P:1:0".to_vec(), b"a".to_vec()),
            (b"P:1:1".to_vec(), b"b".to_vec()),
        ]
    );
    assert!(db.get(b"").expect("empty key").is_none());
}

#[test]
fn rocksdb_round_trip_and_goleveldb_refusal() {
    let dir = TempDir::new("round-trip");
    let hash = {
        let db = RocksDb::open(&dir.path).expect("open rocksdb");
        let store = BlockStore::new(db);
        let (block, parts) = test_block(1, Some(height0_commit()), 4);
        let seen = seen_commit(1);
        store.save_block(&block, &parts, &seen).expect("save");
        block.hash().expect("hash")
    };
    {
        let db = RocksDb::open(&dir.path).expect("reopen rocksdb");
        let store = BlockStore::new(db);
        let loaded = store.load_block(1).expect("reloaded block");
        assert_eq!(loaded.hash().expect("hash"), hash);
        assert!(
            store
                .load_block_meta(1)
                .expect("meta")
                .block_id
                .part_set_header
                .total
                > 1
        );
        assert_eq!(store.load_seen_commit(1).expect("seen").height, 1);
        assert_eq!(store.base(), 1);
        assert_eq!(store.height(), 1);
    }

    let go_dir = TempDir::new("goleveldb");
    fs::write(go_dir.path.join("CURRENT"), b"MANIFEST-000000\n").expect("current");
    fs::write(go_dir.path.join("LOG"), b"").expect("log");
    fs::write(go_dir.path.join("000001.ldb"), b"").expect("ldb");
    let err = match RocksDb::open(&go_dir.path) {
        Ok(_) => panic!("goleveldb directory was opened"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("goleveldb"), "{}", err);
    assert!(matches!(err, Error::GoLevelDb { .. }));
    assert!(!go_dir.path.join("IDENTITY").exists());
}

fn panic_text(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        panic!("unexpected panic payload");
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "eld-tendermint-store-{label}-{}-{nanos}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
