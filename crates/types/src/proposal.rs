//! `types.Proposal` and `ProposalSignBytes`.

use eld_tendermint_proto::types::SignedMsgType;
use prost::Message;

use crate::time::Time;
use crate::vote::{canonical_block_id, check_signature, signed_msg_type_from_i32};
use crate::{BlockId, Error};

/// `types.Proposal`. `pol_round` is -1 when there is no proof-of-lock round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    pub proposal_type: SignedMsgType,
    pub height: i64,
    pub round: i32,
    pub pol_round: i32,
    pub block_id: BlockId,
    pub timestamp: Time,
    pub signature: Vec<u8>,
}

impl Proposal {
    /// Builds a proposal the way `NewProposal` does, without stamping the current time.
    ///
    /// Callers set [`Self::timestamp`]. `NewProposal` in Go uses `tmtime.Now`, which is not
    /// deterministic; pass the time explicitly.
    #[must_use]
    pub fn new(
        height: i64,
        round: i32,
        pol_round: i32,
        block_id: BlockId,
        timestamp: Time,
    ) -> Self {
        Self {
            proposal_type: SignedMsgType::Proposal,
            height,
            round,
            pol_round,
            block_id,
            timestamp,
            signature: Vec::new(),
        }
    }

    /// `Proposal.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when the type, rounds, block id, or signature fail the Go checks.
    /// The block id must be complete.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.proposal_type != SignedMsgType::Proposal {
            return Err(Error::InvalidProposalType);
        }
        if self.height < 0 {
            return Err(Error::NegativeHeight);
        }
        if self.round < 0 {
            return Err(Error::NegativeRound);
        }
        if self.pol_round < -1 {
            return Err(Error::NegativePolRound);
        }
        self.block_id.validate_basic()?;
        if !self.block_id.is_complete() {
            return Err(Error::BlockIdMustBeComplete);
        }
        check_signature(&self.signature)
    }

    /// Length-prefixed canonical protobuf (`ProposalSignBytes`).
    ///
    /// The canonical type is always [`SignedMsgType::Proposal`], even when the struct's
    /// type field differs. A zero block id is omitted.
    #[must_use]
    pub fn sign_bytes(&self, chain_id: &str) -> Vec<u8> {
        self.to_canonical(chain_id).encode_length_delimited_to_vec()
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Proposal {
        eld_tendermint_proto::types::Proposal {
            r#type: self.proposal_type as i32,
            height: self.height,
            round: self.round,
            pol_round: self.pol_round,
            block_id: Some(self.block_id.to_proto()),
            timestamp: Some(self.timestamp.to_prost()),
            signature: self.signature.clone(),
        }
    }

    fn to_canonical(&self, chain_id: &str) -> eld_tendermint_proto::types::CanonicalProposal {
        eld_tendermint_proto::types::CanonicalProposal {
            r#type: SignedMsgType::Proposal as i32,
            height: self.height,
            round: i64::from(self.round),
            pol_round: i64::from(self.pol_round),
            block_id: canonical_block_id(&self.block_id),
            timestamp: Some(self.timestamp.to_prost()),
            chain_id: chain_id.to_owned(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Proposal) -> Result<Self, Error> {
        let proposal = Self {
            proposal_type: signed_msg_type_from_i32(proto.r#type),
            height: proto.height,
            round: proto.round,
            pol_round: proto.pol_round,
            block_id: match &proto.block_id {
                Some(block_id) => BlockId::try_from_proto(block_id)?,
                None => BlockId::default(),
            },
            timestamp: Time::from_prost(proto.timestamp.as_ref()),
            signature: proto.signature.clone(),
        };
        proposal.validate_basic()?;
        Ok(proposal)
    }
}

impl Default for Proposal {
    fn default() -> Self {
        Self {
            proposal_type: SignedMsgType::Unknown,
            height: 0,
            round: 0,
            pol_round: -1,
            block_id: BlockId::default(),
            timestamp: Time::GO_ZERO,
            signature: Vec::new(),
        }
    }
}
