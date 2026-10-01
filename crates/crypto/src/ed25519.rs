//! Ed25519 key bytes in the Go layout (`crypto/ed25519`).
//!
//! A private key is 64 bytes: the 32-byte seed followed by the 32-byte public key.
//! This module does not sign or generate keys. The public key is the stored suffix.

use crate::{Address, Error, address_hash};

/// `ed25519.PubKeySize`.
pub const PUB_KEY_SIZE: usize = 32;

/// `ed25519.PrivateKeySize`.
pub const PRIV_KEY_SIZE: usize = 64;

/// Amino type name for a public key (`ed25519.PubKeyName`).
pub const PUB_KEY_NAME: &str = "tendermint/PubKeyEd25519";

/// Amino type name for a private key (`ed25519.PrivKeyName`).
pub const PRIV_KEY_NAME: &str = "tendermint/PrivKeyEd25519";

/// `ed25519.PubKey`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PubKey([u8; PUB_KEY_SIZE]);

/// `ed25519.PrivKey`. The last 32 bytes are the public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PrivKey([u8; PRIV_KEY_SIZE]);

impl PubKey {
    /// Copies `bytes` when the length is 32.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidLength`] when `bytes` is not 32 bytes long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: [u8; PUB_KEY_SIZE] = bytes.try_into().map_err(|_| Error::InvalidLength {
            expected: PUB_KEY_SIZE,
            got: bytes.len(),
        })?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; PUB_KEY_SIZE] {
        &self.0
    }

    /// `PubKey.Address`: first 20 bytes of `tmhash` of the raw public key.
    #[must_use]
    pub fn address(&self) -> Address {
        address_hash(&self.0)
    }
}

impl PrivKey {
    /// Copies `bytes` when the length is 64.
    ///
    /// Does not check that the public-key suffix is filled in. [`Self::public_key`] does.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidLength`] when `bytes` is not 64 bytes long.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: [u8; PRIV_KEY_SIZE] = bytes.try_into().map_err(|_| Error::InvalidLength {
            expected: PRIV_KEY_SIZE,
            got: bytes.len(),
        })?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8; PRIV_KEY_SIZE] {
        &self.0
    }

    /// Copies bytes `[32..64]`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UninitializedPrivKey`] when that suffix is all zeros.
    /// Go panics in that case (`PubKey`).
    pub fn public_key(&self) -> Result<PubKey, Error> {
        let mut suffix = [0u8; PUB_KEY_SIZE];
        suffix.copy_from_slice(&self.0[PUB_KEY_SIZE..]);
        if suffix == [0; PUB_KEY_SIZE] {
            return Err(Error::UninitializedPrivKey);
        }
        Ok(PubKey(suffix))
    }
}

impl std::fmt::Debug for PubKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PubKey({})", hex_bytes(&self.0))
    }
}

impl std::fmt::Debug for PrivKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrivKey({})", hex_bytes(&self.0))
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
