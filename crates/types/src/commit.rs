//! `types.Commit` and `CommitSig`.

use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};
use prost::Message;

use eld_tendermint_crypto::hash_from_byte_slices;

use crate::time::Time;
use crate::vote::{Vote, check_signature};
use crate::{ADDRESS_SIZE, BlockId, Error, Hash};

fn block_id_flag_from_i32(value: i32) -> BlockIdFlag {
    match value {
        1 => BlockIdFlag::Absent,
        2 => BlockIdFlag::Commit,
        3 => BlockIdFlag::Nil,
        _ => BlockIdFlag::Unknown,
    }
}

/// `types.CommitSig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSig {
    pub block_id_flag: BlockIdFlag,
    pub validator_address: Vec<u8>,
    pub timestamp: Time,
    pub signature: Vec<u8>,
}

impl CommitSig {
    #[must_use]
    pub fn absent() -> Self {
        Self {
            block_id_flag: BlockIdFlag::Absent,
            validator_address: Vec::new(),
            timestamp: Time::GO_ZERO,
            signature: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_absent(&self) -> bool {
        self.block_id_flag == BlockIdFlag::Absent
    }

    /// `CommitSig.ForBlock`. A nil vote is not a vote for the block.
    #[must_use]
    pub fn for_block(&self) -> bool {
        self.block_id_flag == BlockIdFlag::Commit
    }

    /// `CommitSig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown flag, or when an absent sig carries data, or when
    /// a present sig has a bad address or signature.
    pub fn validate_basic(&self) -> Result<(), Error> {
        match self.block_id_flag {
            BlockIdFlag::Absent | BlockIdFlag::Commit | BlockIdFlag::Nil => {}
            BlockIdFlag::Unknown => return Err(Error::UnknownBlockIdFlag),
        }
        if self.block_id_flag == BlockIdFlag::Absent {
            if !self.validator_address.is_empty() {
                return Err(Error::AbsentHasAddress);
            }
            if !self.timestamp.is_zero() {
                return Err(Error::AbsentHasTime);
            }
            if !self.signature.is_empty() {
                return Err(Error::AbsentHasSignature);
            }
            return Ok(());
        }
        if self.validator_address.len() != ADDRESS_SIZE {
            return Err(Error::InvalidAddressLength {
                len: self.validator_address.len(),
            });
        }
        check_signature(&self.signature)
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::CommitSig {
        eld_tendermint_proto::types::CommitSig {
            block_id_flag: self.block_id_flag as i32,
            validator_address: self.validator_address.clone(),
            timestamp: Some(self.timestamp.to_prost()),
            signature: self.signature.clone(),
        }
    }
}

/// `types.Commit`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub height: i64,
    pub round: i32,
    pub block_id: BlockId,
    pub signatures: Vec<CommitSig>,
}

impl Commit {
    /// `Commit.ValidateBasic`. Height 0 skips the block-id and signature checks, as in Go.
    ///
    /// # Errors
    ///
    /// Returns an error for a negative height or round, a nil block at height >= 1, or a bad sig.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.height < 0 {
            return Err(Error::NegativeHeight);
        }
        if self.round < 0 {
            return Err(Error::NegativeRound);
        }
        if self.height >= 1 {
            if self.block_id.is_zero() {
                return Err(Error::CommitForNilBlock);
            }
            if self.signatures.is_empty() {
                return Err(Error::NoCommitSignatures);
            }
            for sig in &self.signatures {
                sig.validate_basic()?;
            }
        }
        Ok(())
    }

    /// `Commit.GetVote`. A `Commit` flag signs `block_id`; absent and nil sign an empty id.
    ///
    /// `None` when `index` is past the last signature.
    #[must_use]
    pub fn vote(&self, index: usize) -> Option<Vote> {
        let sig = self.signatures.get(index)?;
        let block_id = if sig.for_block() {
            self.block_id.clone()
        } else {
            BlockId::default()
        };
        Some(Vote {
            vote_type: SignedMsgType::Precommit,
            height: self.height,
            round: self.round,
            block_id,
            timestamp: sig.timestamp,
            validator_address: sig.validator_address.clone(),
            validator_index: i32::try_from(index).unwrap_or(i32::MAX),
            signature: sig.signature.clone(),
        })
    }

    /// Merkle root of each signature's protobuf encoding.
    #[must_use]
    pub fn hash(&self) -> Hash {
        let leaves: Vec<Vec<u8>> = self
            .signatures
            .iter()
            .map(|sig| sig.to_proto().encode_to_vec())
            .collect();
        Hash::from_array(hash_from_byte_slices(&leaves))
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Commit {
        eld_tendermint_proto::types::Commit {
            height: self.height,
            round: self.round,
            block_id: Some(self.block_id.to_proto()),
            signatures: self.signatures.iter().map(CommitSig::to_proto).collect(),
        }
    }

    /// # Errors
    ///
    /// Returns the same errors as [`Self::validate_basic`] and signature conversion.
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Commit) -> Result<Self, Error> {
        let mut signatures = Vec::with_capacity(proto.signatures.len());
        for sig in &proto.signatures {
            let commit_sig = CommitSig {
                block_id_flag: block_id_flag_from_i32(sig.block_id_flag),
                validator_address: sig.validator_address.clone(),
                timestamp: Time::from_prost(sig.timestamp.as_ref()),
                signature: sig.signature.clone(),
            };
            signatures.push(commit_sig);
        }
        let commit = Self {
            height: proto.height,
            round: proto.round,
            block_id: match &proto.block_id {
                Some(block_id) => BlockId::try_from_proto(block_id)?,
                None => BlockId::default(),
            },
            signatures,
        };
        commit.validate_basic()?;
        Ok(commit)
    }
}
