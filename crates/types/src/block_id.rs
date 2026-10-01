//! `PartSetHeader` and `BlockID`.

use prost::Message;

use crate::{Error, validate_hash};

/// `types.PartSetHeader`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PartSetHeader {
    pub total: u32,
    pub hash: Vec<u8>,
}

impl PartSetHeader {
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.total == 0 && self.hash.is_empty()
    }

    /// # Errors
    ///
    /// Returns [`Error::InvalidHashLength`] when `hash` is neither empty nor 32 bytes.
    pub fn validate_basic(&self) -> Result<(), Error> {
        validate_hash(&self.hash)
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::PartSetHeader {
        eld_tendermint_proto::types::PartSetHeader {
            total: self.total,
            hash: self.hash.clone(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::PartSetHeader,
    ) -> Result<Self, Error> {
        let header = Self {
            total: proto.total,
            hash: proto.hash.clone(),
        };
        header.validate_basic()?;
        Ok(header)
    }
}

/// `types.BlockID`. `hash` is the header hash of the block.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BlockId {
    pub hash: Vec<u8>,
    pub part_set_header: PartSetHeader,
}

impl BlockId {
    /// True when this is the id of a nil block.
    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.hash.is_empty() && self.part_set_header.is_zero()
    }

    /// True when both hashes are 32 bytes and the part set is non-empty.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.hash.len() == crate::HASH_SIZE
            && self.part_set_header.total > 0
            && self.part_set_header.hash.len() == crate::HASH_SIZE
    }

    /// # Errors
    ///
    /// Returns [`Error::InvalidHashLength`] when either hash has a forbidden length.
    pub fn validate_basic(&self) -> Result<(), Error> {
        validate_hash(&self.hash)?;
        self.part_set_header.validate_basic()
    }

    /// Protobuf `BlockID`. `part_set_header` is always present (`gogoproto.nullable = false`).
    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::BlockId {
        eld_tendermint_proto::types::BlockId {
            hash: self.hash.clone(),
            part_set_header: Some(self.part_set_header.to_proto()),
        }
    }

    #[must_use]
    pub fn to_canonical(&self) -> eld_tendermint_proto::types::CanonicalBlockId {
        eld_tendermint_proto::types::CanonicalBlockId {
            hash: self.hash.clone(),
            part_set_header: Some(eld_tendermint_proto::types::CanonicalPartSetHeader {
                total: self.part_set_header.total,
                hash: self.part_set_header.hash.clone(),
            }),
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.to_proto().encode_to_vec()
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::BlockId) -> Result<Self, Error> {
        let part_set_header = match &proto.part_set_header {
            Some(header) => PartSetHeader::try_from_proto(header)?,
            None => PartSetHeader::default(),
        };
        let block_id = Self {
            hash: proto.hash.clone(),
            part_set_header,
        };
        block_id.validate_basic()?;
        Ok(block_id)
    }
}
