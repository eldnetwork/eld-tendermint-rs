//! `types.Header`.

use prost::Message;

use crate::cdc::{cdc_encode_bytes, cdc_encode_i64, cdc_encode_string};
use crate::time::Time;
use crate::{BLOCK_PROTOCOL, BlockId, ChainId, Error, Hash, validate_hash};
use eld_tendermint_crypto::hash_from_byte_slices;

/// `version.Consensus` stored on a header (`Block` protocol and `App` version).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConsensusVersion {
    pub block: u64,
    pub app: u64,
}

impl ConsensusVersion {
    #[must_use]
    pub fn encode(self) -> Vec<u8> {
        eld_tendermint_proto::version::Consensus {
            block: self.block,
            app: self.app,
        }
        .encode_to_vec()
    }
}

impl Default for ConsensusVersion {
    fn default() -> Self {
        Self {
            block: BLOCK_PROTOCOL,
            app: 0,
        }
    }
}

/// Block header. `hash` is the Merkle root of the fields in struct order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    pub version: ConsensusVersion,
    pub chain_id: ChainId,
    pub height: i64,
    pub time: Time,
    pub last_block_id: BlockId,
    pub last_commit_hash: Vec<u8>,
    pub data_hash: Vec<u8>,
    pub validators_hash: Vec<u8>,
    pub next_validators_hash: Vec<u8>,
    pub consensus_hash: Vec<u8>,
    pub app_hash: Vec<u8>,
    pub last_results_hash: Vec<u8>,
    pub evidence_hash: Vec<u8>,
    pub proposer_address: Vec<u8>,
}

impl Header {
    /// `Header.ValidateBasic`. Time is not checked.
    ///
    /// # Errors
    ///
    /// Returns an error when the protocol, chain id, height, block id, hashes, or
    /// proposer address fail the Go checks. `app_hash` may be any length.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.version.block != BLOCK_PROTOCOL {
            return Err(Error::WrongBlockProtocol {
                got: self.version.block,
            });
        }
        self.chain_id.validate_basic()?;
        if self.height < 0 {
            return Err(Error::NegativeHeight);
        }
        if self.height == 0 {
            return Err(Error::ZeroHeight);
        }
        self.last_block_id.validate_basic()?;
        validate_hash(&self.last_commit_hash)?;
        validate_hash(&self.data_hash)?;
        validate_hash(&self.evidence_hash)?;
        if self.proposer_address.len() != crate::ADDRESS_SIZE {
            return Err(Error::InvalidAddressLength {
                len: self.proposer_address.len(),
            });
        }
        validate_hash(&self.validators_hash)?;
        validate_hash(&self.next_validators_hash)?;
        validate_hash(&self.consensus_hash)?;
        validate_hash(&self.last_results_hash)?;
        Ok(())
    }

    /// `Header.Hash`. `None` when `validators_hash` is empty, matching Go's nil return.
    #[must_use]
    pub fn hash(&self) -> Option<Hash> {
        if self.validators_hash.is_empty() {
            return None;
        }
        let leaves = [
            self.version.encode(),
            cdc_encode_string(self.chain_id.as_str()),
            cdc_encode_i64(self.height),
            self.time.encode(),
            self.last_block_id.encode(),
            cdc_encode_bytes(&self.last_commit_hash),
            cdc_encode_bytes(&self.data_hash),
            cdc_encode_bytes(&self.validators_hash),
            cdc_encode_bytes(&self.next_validators_hash),
            cdc_encode_bytes(&self.consensus_hash),
            cdc_encode_bytes(&self.app_hash),
            cdc_encode_bytes(&self.last_results_hash),
            cdc_encode_bytes(&self.evidence_hash),
            cdc_encode_bytes(&self.proposer_address),
        ];
        Some(Hash::from_array(hash_from_byte_slices(&leaves)))
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Header {
        eld_tendermint_proto::types::Header {
            version: Some(eld_tendermint_proto::version::Consensus {
                block: self.version.block,
                app: self.version.app,
            }),
            chain_id: self.chain_id.as_str().to_owned(),
            height: self.height,
            time: Some(self.time.to_prost()),
            last_block_id: Some(self.last_block_id.to_proto()),
            last_commit_hash: self.last_commit_hash.clone(),
            data_hash: self.data_hash.clone(),
            validators_hash: self.validators_hash.clone(),
            next_validators_hash: self.next_validators_hash.clone(),
            consensus_hash: self.consensus_hash.clone(),
            app_hash: self.app_hash.clone(),
            last_results_hash: self.last_results_hash.clone(),
            evidence_hash: self.evidence_hash.clone(),
            proposer_address: self.proposer_address.clone(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`], plus block-id errors.
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Header) -> Result<Self, Error> {
        let version = proto.version.unwrap_or_default();
        let last_block_id = match &proto.last_block_id {
            Some(block_id) => BlockId::try_from_proto(block_id)?,
            None => BlockId::default(),
        };
        let header = Self {
            version: ConsensusVersion {
                block: version.block,
                app: version.app,
            },
            chain_id: ChainId::new(proto.chain_id.clone()),
            height: proto.height,
            time: Time::from_prost(proto.time.as_ref()),
            last_block_id,
            last_commit_hash: proto.last_commit_hash.clone(),
            data_hash: proto.data_hash.clone(),
            validators_hash: proto.validators_hash.clone(),
            next_validators_hash: proto.next_validators_hash.clone(),
            consensus_hash: proto.consensus_hash.clone(),
            app_hash: proto.app_hash.clone(),
            last_results_hash: proto.last_results_hash.clone(),
            evidence_hash: proto.evidence_hash.clone(),
            proposer_address: proto.proposer_address.clone(),
        };
        header.validate_basic()?;
        Ok(header)
    }
}
