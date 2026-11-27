//! Replaceable chain summary.
//!
//! [`SummaryHeader`] is the small record kept in memory. [`SummaryBody`] is the
//! compressed application snapshot. After zstd decompression those bytes are
//! [`eld_tendermint_proto::types::AppStateSnapshot`]. `body_hash` is the tmhash
//! of the compressed snapshot bytes. `summary_hash` is the tmhash of the header protobuf with `summary_hash`
//! left empty, so the digest covers `body_hash` and every other field.

use prost::Message;

use eld_tendermint_crypto::sum;

use crate::{BlockId, ConsensusParams, Error, Hash, ValidatorSet, hash_consensus_params};

/// Fields a caller supplies to [`SummaryHeader::build`].
///
/// The set hashes, params hash, body hash, and summary hash are computed there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryInput {
    pub end_height: i64,
    pub last_block_id: BlockId,
    pub last_results_hash: Hash,
    pub app_hash: Hash,
    pub last_height_validators_changed: i64,
    pub last_height_consensus_params_changed: i64,
    pub validators: ValidatorSet,
    pub next_validators: ValidatorSet,
    pub consensus_params: ConsensusParams,
}

/// Small record of the chain summary at `end_height`.
///
/// `validators_hash`, `next_validators_hash`, and `consensus_hash` are computed
/// from the sets and params. A load that does not recompute to those stored
/// hashes is rejected, as is a `summary_hash` that does not match the header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryHeader {
    pub end_height: i64,
    pub summary_hash: Hash,
    pub last_block_id: BlockId,
    pub last_results_hash: Hash,
    pub app_hash: Hash,
    pub body_hash: Hash,
    pub validators_hash: Hash,
    pub next_validators_hash: Hash,
    pub consensus_hash: Hash,
    pub last_height_validators_changed: i64,
    pub last_height_consensus_params_changed: i64,
    pub validators: ValidatorSet,
    pub next_validators: ValidatorSet,
    pub consensus_params: ConsensusParams,
}

/// Compressed application-state snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryBody {
    pub state_snapshot: Vec<u8>,
}

impl SummaryHeader {
    /// Fills the set, params, body, and summary hashes.
    ///
    /// `body` is the compressed snapshot. Its tmhash becomes `body_hash`.
    ///
    /// # Errors
    ///
    /// Returns a validator-set or params error, [`Error::ZeroHeight`] when
    /// `end_height` is 0, or [`Error::BlockIdMustBeComplete`] when `last_block_id`
    /// is not a complete block id.
    pub fn build(input: SummaryInput, body: &[u8]) -> Result<Self, Error> {
        let validators_hash = input.validators.hash()?;
        let next_validators_hash = input.next_validators.hash()?;
        let consensus_hash = hash_consensus_params(&input.consensus_params);
        let header = Self {
            end_height: input.end_height,
            summary_hash: Hash::from_array([0; crate::HASH_SIZE]),
            last_block_id: input.last_block_id,
            last_results_hash: input.last_results_hash,
            app_hash: input.app_hash,
            body_hash: hash_snapshot(body),
            validators_hash,
            next_validators_hash,
            consensus_hash,
            last_height_validators_changed: input.last_height_validators_changed,
            last_height_consensus_params_changed: input.last_height_consensus_params_changed,
            validators: input.validators,
            next_validators: input.next_validators,
            consensus_params: input.consensus_params,
        };
        let summary_hash = header.summary_hash_of_header()?;
        let header = Self {
            summary_hash,
            ..header
        };
        header.check()?;
        Ok(header)
    }

    /// Protobuf `tendermint.types.SummaryHeader`.
    ///
    /// # Errors
    ///
    /// Returns a validator proto error.
    pub fn to_proto(&self) -> Result<eld_tendermint_proto::types::SummaryHeader, Error> {
        Ok(eld_tendermint_proto::types::SummaryHeader {
            end_height: self.end_height,
            summary_hash: self.summary_hash.as_bytes().to_vec(),
            last_block_id: Some(self.last_block_id.to_proto()),
            last_results_hash: self.last_results_hash.as_bytes().to_vec(),
            app_hash: self.app_hash.as_bytes().to_vec(),
            body_hash: self.body_hash.as_bytes().to_vec(),
            validators_hash: self.validators_hash.as_bytes().to_vec(),
            next_validators_hash: self.next_validators_hash.as_bytes().to_vec(),
            consensus_hash: self.consensus_hash.as_bytes().to_vec(),
            last_height_validators_changed: self.last_height_validators_changed,
            last_height_consensus_params_changed: self.last_height_consensus_params_changed,
            validators: Some(self.validators.to_proto()?),
            next_validators: Some(self.next_validators.to_proto()?),
            consensus_params: Some(self.consensus_params.to_proto()),
        })
    }

    /// # Errors
    ///
    /// Returns a nested decode error, or a hash error when the stored
    /// `validators_hash`, `next_validators_hash`, `consensus_hash`, or
    /// `summary_hash` does not match a fresh computation.
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::SummaryHeader,
    ) -> Result<Self, Error> {
        let Some(last_block_id) = proto.last_block_id.as_ref() else {
            return Err(Error::BlockIdMustBeComplete);
        };
        let Some(validators) = proto.validators.as_ref() else {
            return Err(Error::MissingSummaryValidators);
        };
        let Some(next_validators) = proto.next_validators.as_ref() else {
            return Err(Error::MissingSummaryNextValidators);
        };
        let Some(consensus_params) = proto.consensus_params.as_ref() else {
            return Err(Error::MissingSummaryConsensusParams);
        };
        let header = Self {
            end_height: proto.end_height,
            summary_hash: Hash::from_bytes(&proto.summary_hash)?,
            last_block_id: BlockId::try_from_proto(last_block_id)?,
            last_results_hash: Hash::from_bytes(&proto.last_results_hash)?,
            app_hash: Hash::from_bytes(&proto.app_hash)?,
            body_hash: Hash::from_bytes(&proto.body_hash)?,
            validators_hash: Hash::from_bytes(&proto.validators_hash)?,
            next_validators_hash: Hash::from_bytes(&proto.next_validators_hash)?,
            consensus_hash: Hash::from_bytes(&proto.consensus_hash)?,
            last_height_validators_changed: proto.last_height_validators_changed,
            last_height_consensus_params_changed: proto.last_height_consensus_params_changed,
            validators: ValidatorSet::try_from_proto(validators)?,
            next_validators: ValidatorSet::try_from_proto(next_validators)?,
            consensus_params: ConsensusParams::try_from_proto(consensus_params)?,
        };
        header.check()?;
        Ok(header)
    }

    /// tmhash of this header encoded with `summary_hash` empty.
    fn summary_hash_of_header(&self) -> Result<Hash, Error> {
        let mut proto = self.to_proto()?;
        proto.summary_hash.clear();
        Ok(Hash::from_array(sum(&proto.encode_to_vec())))
    }

    fn check(&self) -> Result<(), Error> {
        if self.end_height == 0 {
            return Err(Error::ZeroHeight);
        }
        if !self.last_block_id.is_complete() {
            return Err(Error::BlockIdMustBeComplete);
        }
        self.validators.validate_basic()?;
        self.next_validators.validate_basic()?;
        self.consensus_params.validate()?;
        if self.validators.hash()? != self.validators_hash {
            return Err(Error::SummaryValidatorsHashMismatch);
        }
        if self.next_validators.hash()? != self.next_validators_hash {
            return Err(Error::SummaryNextValidatorsHashMismatch);
        }
        if hash_consensus_params(&self.consensus_params) != self.consensus_hash {
            return Err(Error::SummaryConsensusHashMismatch);
        }
        if self.summary_hash_of_header()? != self.summary_hash {
            return Err(Error::SummaryHashMismatch);
        }
        Ok(())
    }
}

impl SummaryBody {
    /// tmhash of the compressed snapshot bytes.
    #[must_use]
    pub fn hash(&self) -> Hash {
        hash_snapshot(&self.state_snapshot)
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::SummaryBody {
        eld_tendermint_proto::types::SummaryBody {
            state_snapshot: self.state_snapshot.clone(),
        }
    }

    #[must_use]
    pub fn from_proto(proto: &eld_tendermint_proto::types::SummaryBody) -> Self {
        Self {
            state_snapshot: proto.state_snapshot.clone(),
        }
    }
}

fn hash_snapshot(bytes: &[u8]) -> Hash {
    Hash::from_array(sum(bytes))
}
