//! `types.Tx` and `types.Txs`.

use eld_tendermint_crypto::{hash_from_byte_slices, sum};

use crate::Hash;

/// Raw transaction bytes. There is no `ValidateBasic`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tx(Vec<u8>);

impl Tx {
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// `Tx.Hash`: tmhash of the raw bytes.
    #[must_use]
    pub fn hash(&self) -> Hash {
        Hash::from_array(sum(&self.0))
    }
}

impl From<&[u8]> for Tx {
    fn from(bytes: &[u8]) -> Self {
        Self::new(bytes.to_vec())
    }
}

impl From<Vec<u8>> for Tx {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

/// Ordered transactions. `hash` is the header `data_hash`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Txs(Vec<Tx>);

impl Txs {
    #[must_use]
    pub fn new(txs: Vec<Tx>) -> Self {
        Self(txs)
    }

    #[must_use]
    pub fn as_slice(&self) -> &[Tx] {
        &self.0
    }

    /// Merkle root of the individual transaction hashes. An empty list hashes as an empty tree.
    #[must_use]
    pub fn hash(&self) -> Hash {
        let leaves: Vec<Hash> = self.0.iter().map(Tx::hash).collect();
        Hash::from_array(hash_from_byte_slices(&leaves))
    }
}
