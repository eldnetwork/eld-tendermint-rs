//! `types.Vote` and `VoteSignBytes`.

use eld_tendermint_proto::types::SignedMsgType;
use prost::Message;

use crate::time::Time;
use crate::{ADDRESS_SIZE, BlockId, Error, MAX_SIGNATURE_SIZE};

/// `types.Vote`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vote {
    pub vote_type: SignedMsgType,
    pub height: i64,
    pub round: i32,
    pub block_id: BlockId,
    pub timestamp: Time,
    pub validator_address: Vec<u8>,
    pub validator_index: i32,
    pub signature: Vec<u8>,
}

impl Default for Vote {
    fn default() -> Self {
        Self {
            vote_type: SignedMsgType::Unknown,
            height: 0,
            round: 0,
            block_id: BlockId::default(),
            timestamp: Time::GO_ZERO,
            validator_address: Vec::new(),
            validator_index: 0,
            signature: Vec::new(),
        }
    }
}

impl Vote {
    /// `Vote.ValidateBasic`. Does not check the signature cryptographically.
    ///
    /// # Errors
    ///
    /// Returns an error when the type, height, round, block id, address, index, or
    /// signature fail the Go checks.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if !is_vote_type(self.vote_type) {
            return Err(Error::InvalidVoteType);
        }
        if self.height < 0 {
            return Err(Error::NegativeHeight);
        }
        if self.round < 0 {
            return Err(Error::NegativeRound);
        }
        self.block_id.validate_basic()?;
        if !self.block_id.is_zero() && !self.block_id.is_complete() {
            return Err(Error::BlockIdMustBeComplete);
        }
        if self.validator_address.len() != ADDRESS_SIZE {
            return Err(Error::InvalidAddressLength {
                len: self.validator_address.len(),
            });
        }
        if self.validator_index < 0 {
            return Err(Error::NegativeValidatorIndex);
        }
        check_signature(&self.signature)
    }

    /// Length-prefixed canonical protobuf (`VoteSignBytes`).
    ///
    /// Validator address and index are omitted. A zero block id is omitted.
    /// The timestamp is always encoded, including Go's zero time.
    #[must_use]
    pub fn sign_bytes(&self, chain_id: &str) -> Vec<u8> {
        self.to_canonical(chain_id).encode_length_delimited_to_vec()
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Vote {
        eld_tendermint_proto::types::Vote {
            r#type: self.vote_type as i32,
            height: self.height,
            round: self.round,
            block_id: Some(self.block_id.to_proto()),
            timestamp: Some(self.timestamp.to_prost()),
            validator_address: self.validator_address.clone(),
            validator_index: self.validator_index,
            signature: self.signature.clone(),
        }
    }

    fn to_canonical(&self, chain_id: &str) -> eld_tendermint_proto::types::CanonicalVote {
        eld_tendermint_proto::types::CanonicalVote {
            r#type: self.vote_type as i32,
            height: self.height,
            round: i64::from(self.round),
            block_id: canonical_block_id(&self.block_id),
            timestamp: Some(self.timestamp.to_prost()),
            chain_id: chain_id.to_owned(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Vote) -> Result<Self, Error> {
        let vote = Self {
            vote_type: signed_msg_type_from_i32(proto.r#type),
            height: proto.height,
            round: proto.round,
            block_id: match &proto.block_id {
                Some(block_id) => BlockId::try_from_proto(block_id)?,
                None => BlockId::default(),
            },
            timestamp: Time::from_prost(proto.timestamp.as_ref()),
            validator_address: proto.validator_address.clone(),
            validator_index: proto.validator_index,
            signature: proto.signature.clone(),
        };
        vote.validate_basic()?;
        Ok(vote)
    }
}

pub(crate) fn signed_msg_type_from_i32(value: i32) -> SignedMsgType {
    match value {
        1 => SignedMsgType::Prevote,
        2 => SignedMsgType::Precommit,
        32 => SignedMsgType::Proposal,
        _ => SignedMsgType::Unknown,
    }
}

pub(crate) fn is_vote_type(vote_type: SignedMsgType) -> bool {
    matches!(vote_type, SignedMsgType::Prevote | SignedMsgType::Precommit)
}

pub(crate) fn check_signature(signature: &[u8]) -> Result<(), Error> {
    if signature.is_empty() {
        return Err(Error::MissingSignature);
    }
    if signature.len() > MAX_SIGNATURE_SIZE {
        return Err(Error::SignatureTooBig {
            len: signature.len(),
        });
    }
    Ok(())
}

pub(crate) fn canonical_block_id(
    block_id: &BlockId,
) -> Option<eld_tendermint_proto::types::CanonicalBlockId> {
    if block_id.is_zero() {
        None
    } else {
        Some(block_id.to_canonical())
    }
}
