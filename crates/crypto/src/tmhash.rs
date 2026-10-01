//! SHA-256 as Tendermint uses it (`crypto/tmhash`).
//!
//! `sum` is SHA-256. `sum_truncated` is the first 20 bytes, which is an address.

use sha2::{Digest, Sha256};

/// `tmhash.Size`.
pub const SIZE: usize = 32;

/// `tmhash.TruncatedSize`.
pub const TRUNCATED_SIZE: usize = 20;

/// `tmhash.Sum`.
#[must_use]
pub fn sum(bytes: &[u8]) -> [u8; SIZE] {
    Sha256::digest(bytes).into()
}

/// `tmhash.SumTruncated`: first 20 bytes of SHA-256.
#[must_use]
pub fn sum_truncated(bytes: &[u8]) -> [u8; TRUNCATED_SIZE] {
    let full = sum(bytes);
    let mut out = [0u8; TRUNCATED_SIZE];
    out.copy_from_slice(&full[..TRUNCATED_SIZE]);
    out
}
