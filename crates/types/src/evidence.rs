//! `types.DuplicateVoteEvidence` and `EvidenceList`.
//!
//! `ValidateBasic` checks the two votes and that their block-id keys are strictly ordered.
//! Conflicting height, round, type, address, block id, power, and signatures are
//! [`DuplicateVoteEvidence::verify`], matching `evidence.VerifyDuplicateVote`.

use prost::Message;

use eld_tendermint_crypto::{hash_from_byte_slices, sum};

use crate::{Error, Hash, Time, ValidatorSet, Vote};

/// `types.DuplicateVoteEvidence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DuplicateVoteEvidence {
    pub vote_a: Vote,
    pub vote_b: Vote,
    pub total_voting_power: i64,
    pub validator_power: i64,
    pub timestamp: Time,
}

impl DuplicateVoteEvidence {
    /// `NewDuplicateVoteEvidence`.
    ///
    /// Looks up `vote_a`'s address in `validator_set` before ordering. The vote with the
    /// smaller [`BlockId::key`] becomes `vote_a`. Powers come from the set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ValidatorNotInSet`] when `vote_a`'s address is missing. Go returns nil.
    pub fn new(
        mut vote_a: Vote,
        mut vote_b: Vote,
        timestamp: Time,
        validator_set: &ValidatorSet,
    ) -> Result<Self, Error> {
        let validator = validator_set
            .validators()
            .iter()
            .find(|validator| validator.address == vote_a.validator_address)
            .ok_or(Error::ValidatorNotInSet)?;
        if vote_a.block_id.key() >= vote_b.block_id.key() {
            std::mem::swap(&mut vote_a, &mut vote_b);
        }
        Ok(Self {
            vote_a,
            vote_b,
            total_voting_power: validator_set.total_voting_power(),
            validator_power: validator.voting_power,
            timestamp,
        })
    }

    /// `DuplicateVoteEvidence.ValidateBasic`.
    ///
    /// Does not require the votes to share height, round, type, or address.
    ///
    /// # Errors
    ///
    /// Returns a vote error, or [`Error::DuplicateVoteOrder`] when `vote_a`'s block-id key
    /// is not strictly less than `vote_b`'s.
    pub fn validate_basic(&self) -> Result<(), Error> {
        self.vote_a.validate_basic()?;
        self.vote_b.validate_basic()?;
        if self.vote_a.block_id.key() >= self.vote_b.block_id.key() {
            return Err(Error::DuplicateVoteOrder);
        }
        Ok(())
    }

    /// `VerifyDuplicateVote`.
    ///
    /// # Errors
    ///
    /// Returns an error when the validator is missing, the votes do not conflict, the
    /// recorded powers differ from the set, or either signature fails.
    pub fn verify(&self, chain_id: &str, validator_set: &ValidatorSet) -> Result<(), Error> {
        let validator = validator_set
            .validators()
            .iter()
            .find(|validator| validator.address == self.vote_a.validator_address)
            .ok_or(Error::ValidatorNotInSet)?;
        if self.vote_a.height != self.vote_b.height
            || self.vote_a.round != self.vote_b.round
            || self.vote_a.vote_type != self.vote_b.vote_type
        {
            return Err(Error::EvidenceHeightRoundTypeMismatch);
        }
        if self.vote_a.validator_address != self.vote_b.validator_address {
            return Err(Error::EvidenceAddressMismatch);
        }
        if self.vote_a.block_id == self.vote_b.block_id {
            return Err(Error::EvidenceSameBlockId);
        }
        let pub_key = validator.pub_key.as_ref().ok_or(Error::MissingPubKey)?;
        if pub_key.address().as_slice() != self.vote_a.validator_address.as_slice() {
            return Err(Error::EvidencePubKeyMismatch);
        }
        if validator.voting_power != self.validator_power {
            return Err(Error::EvidenceValidatorPowerMismatch);
        }
        if validator_set.total_voting_power() != self.total_voting_power {
            return Err(Error::EvidenceTotalPowerMismatch);
        }
        self.vote_a.verify_signature(pub_key, chain_id)?;
        self.vote_b.verify_signature(pub_key, chain_id)?;
        Ok(())
    }

    /// `DuplicateVoteEvidence.Bytes`: the marshaled evidence proto, not the `Evidence` oneof.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        self.to_proto().encode_to_vec()
    }

    /// `DuplicateVoteEvidence.Hash`: `tmhash` of [`Self::bytes`].
    #[must_use]
    pub fn hash(&self) -> Hash {
        Hash::from_array(sum(&self.bytes()))
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::DuplicateVoteEvidence {
        eld_tendermint_proto::types::DuplicateVoteEvidence {
            vote_a: Some(self.vote_a.to_proto()),
            vote_b: Some(self.vote_b.to_proto()),
            total_voting_power: self.total_voting_power,
            validator_power: self.validator_power,
            timestamp: Some(self.timestamp.to_prost()),
        }
    }

    /// `EvidenceToProto`: the oneof wrapper a block stores.
    #[must_use]
    pub fn to_evidence_proto(&self) -> eld_tendermint_proto::types::Evidence {
        eld_tendermint_proto::types::Evidence {
            sum: Some(
                eld_tendermint_proto::types::evidence::Sum::DuplicateVoteEvidence(self.to_proto()),
            ),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::MissingEvidenceVote`], a vote error, or [`Error::DuplicateVoteOrder`].
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::DuplicateVoteEvidence,
    ) -> Result<Self, Error> {
        let Some(vote_a) = proto.vote_a.as_ref() else {
            return Err(Error::MissingEvidenceVote);
        };
        let Some(vote_b) = proto.vote_b.as_ref() else {
            return Err(Error::MissingEvidenceVote);
        };
        let evidence = Self {
            vote_a: Vote::try_from_proto(vote_a)?,
            vote_b: Vote::try_from_proto(vote_b)?,
            total_voting_power: proto.total_voting_power,
            validator_power: proto.validator_power,
            timestamp: Time::from_prost(proto.timestamp.as_ref()),
        };
        evidence.validate_basic()?;
        Ok(evidence)
    }

    /// # Errors
    ///
    /// Returns [`Error::UnsupportedEvidence`] for a light-client attack or a missing sum,
    /// otherwise the same errors as [`Self::try_from_proto`].
    pub fn try_from_evidence_proto(
        proto: &eld_tendermint_proto::types::Evidence,
    ) -> Result<Self, Error> {
        match &proto.sum {
            Some(eld_tendermint_proto::types::evidence::Sum::DuplicateVoteEvidence(evidence)) => {
                Self::try_from_proto(evidence)
            }
            Some(eld_tendermint_proto::types::evidence::Sum::LightClientAttackEvidence(_))
            | None => Err(Error::UnsupportedEvidence),
        }
    }
}

/// `types.EvidenceList`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvidenceList {
    pub evidence: Vec<DuplicateVoteEvidence>,
}

impl EvidenceList {
    #[must_use]
    pub fn new(evidence: Vec<DuplicateVoteEvidence>) -> Self {
        Self { evidence }
    }

    /// `EvidenceList.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns the first item's [`DuplicateVoteEvidence::validate_basic`] error.
    pub fn validate_basic(&self) -> Result<(), Error> {
        for evidence in &self.evidence {
            evidence.validate_basic()?;
        }
        Ok(())
    }

    /// `EvidenceList.Hash`: merkle root of each item's [`DuplicateVoteEvidence::bytes`].
    ///
    /// An empty list is the empty Merkle root.
    #[must_use]
    pub fn hash(&self) -> Hash {
        let bytes: Vec<Vec<u8>> = self
            .evidence
            .iter()
            .map(DuplicateVoteEvidence::bytes)
            .collect();
        Hash::from_array(hash_from_byte_slices(&bytes))
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::EvidenceList {
        eld_tendermint_proto::types::EvidenceList {
            evidence: self
                .evidence
                .iter()
                .map(DuplicateVoteEvidence::to_evidence_proto)
                .collect(),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::UnsupportedEvidence`] for a light-client attack, or an item error.
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::EvidenceList,
    ) -> Result<Self, Error> {
        let mut evidence = Vec::with_capacity(proto.evidence.len());
        for item in &proto.evidence {
            evidence.push(DuplicateVoteEvidence::try_from_evidence_proto(item)?);
        }
        Ok(Self { evidence })
    }
}
