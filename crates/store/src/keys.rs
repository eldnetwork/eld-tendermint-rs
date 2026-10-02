//! Key bytes copied from `store/store.go`.
//!
//! Height and part index use the decimal text Go's `fmt.Sprintf("%v")` produces
//! for an integer. Hash hex is lowercase, with no `0x` prefix (`%x`).

/// `blockStore`. Prost `tendermint.store.BlockStoreState`.
pub(crate) const BLOCK_STORE_KEY: &[u8] = b"blockStore";

/// `H:{height}`.
pub(crate) fn block_meta_key(height: i64) -> Vec<u8> {
    format!("H:{height}").into_bytes()
}

/// `P:{height}:{index}`.
pub(crate) fn block_part_key(height: i64, index: u32) -> Vec<u8> {
    format!("P:{height}:{index}").into_bytes()
}

/// `C:{height}`.
pub(crate) fn block_commit_key(height: i64) -> Vec<u8> {
    format!("C:{height}").into_bytes()
}

/// `SC:{height}`.
pub(crate) fn seen_commit_key(height: i64) -> Vec<u8> {
    format!("SC:{height}").into_bytes()
}

/// `BH:{hex}`. An empty hash is the key `BH:`, matching Go `%x` of a nil slice.
pub(crate) fn block_hash_key(hash: &[u8]) -> Vec<u8> {
    format!("BH:{}", hex::encode(hash)).into_bytes()
}
