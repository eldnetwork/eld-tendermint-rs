//! `Hash` and `ChainID`.
//!
//! Go stores hashes as `[]byte`. A hash is absent when the slice is empty and
//! invalid when any other length is not 32. Computed roots use [`Hash`].

use crate::{Error, HASH_SIZE, MAX_CHAIN_ID_LEN};

/// 32-byte tmhash digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash([u8; HASH_SIZE]);

impl Hash {
    #[must_use]
    pub const fn from_array(bytes: [u8; HASH_SIZE]) -> Self {
        Self(bytes)
    }

    /// Copies `bytes` when the length is 32.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHashLength`] when `bytes` is not 32 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: [u8; HASH_SIZE] = bytes
            .try_into()
            .map_err(|_| Error::InvalidHashLength { len: bytes.len() })?;
        Ok(Self(bytes))
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; HASH_SIZE] {
        &self.0
    }
}

impl std::fmt::Debug for Hash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Hash({})", hex::encode_upper(self.0))
    }
}

impl AsRef<[u8]> for Hash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// `types.ValidateHash`: empty is allowed, any other length must be 32.
///
/// # Errors
///
/// Returns [`Error::InvalidHashLength`] when `bytes` is neither empty nor 32 bytes.
pub fn validate_hash(bytes: &[u8]) -> Result<(), Error> {
    if bytes.is_empty() || bytes.len() == HASH_SIZE {
        Ok(())
    } else {
        Err(Error::InvalidHashLength { len: bytes.len() })
    }
}

/// Chain identifier. Header checks the maximum length. Genesis also rejects empty.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ChainId(String);

impl ChainId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Header rule: length at most [`MAX_CHAIN_ID_LEN`](crate::MAX_CHAIN_ID_LEN). Empty is allowed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ChainIdTooLong`] when the byte length exceeds 50.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.0.len() > MAX_CHAIN_ID_LEN {
            Err(Error::ChainIdTooLong { len: self.0.len() })
        } else {
            Ok(())
        }
    }

    /// Genesis rule: non-empty and at most 50 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ChainIdEmpty`] or [`Error::ChainIdTooLong`].
    pub fn validate_genesis(&self) -> Result<(), Error> {
        if self.0.is_empty() {
            return Err(Error::ChainIdEmpty);
        }
        self.validate_basic()
    }
}

impl From<&str> for ChainId {
    fn from(id: &str) -> Self {
        Self::new(id)
    }
}

impl std::fmt::Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
