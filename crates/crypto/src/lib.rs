//! Ed25519 key bytes, tmhash, Merkle roots, and Amino JSON for Tendermint 0.34.
//!
//! Matches `crypto/crypto.go`, `crypto/tmhash`, `crypto/merkle`, and `crypto/ed25519`
//! in the Go tree. Signing and the other curves are later. Merkle proofs cover inclusion
//! only; value-ops and key paths are later.

mod amino;
mod ed25519;
mod encoding;
mod ensured;
mod merkle;
mod tmhash;

pub mod error;

pub use amino::{marshal_priv_key, marshal_pub_key, unmarshal_priv_key, unmarshal_pub_key};
pub use ed25519::{PRIV_KEY_NAME, PRIV_KEY_SIZE, PUB_KEY_NAME, PUB_KEY_SIZE, PrivKey, PubKey};
pub use encoding::{pub_key_from_proto, pub_key_to_proto};
pub use error::Error;
pub use merkle::{MAX_AUNTS, Proof, hash_from_byte_slices, proofs_from_byte_slices};
pub use tmhash::{SIZE as TMHASH_SIZE, TRUNCATED_SIZE, sum, sum_truncated};

/// Length of a Tendermint address. Same as `crypto.AddressSize` / `tmhash.TruncatedSize`.
pub const ADDRESS_SIZE: usize = TRUNCATED_SIZE;

/// First 20 bytes of `tmhash(pubkey)`. Go stores this as `crypto.Address` (`bytes.HexBytes`).
pub type Address = [u8; ADDRESS_SIZE];

/// `crypto.AddressHash`: truncated SHA-256 of `bytes`.
#[must_use]
pub fn address_hash(bytes: &[u8]) -> Address {
    sum_truncated(bytes)
}
