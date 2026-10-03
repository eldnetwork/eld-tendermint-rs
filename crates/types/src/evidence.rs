//! `types.DuplicateVoteEvidence`, `types.LightClientAttackEvidence`, and `EvidenceList`.
//!
//! `ValidateBasic` checks the two votes and that their block-id keys are strictly ordered.
//! Conflicting height, round, type, address, block id, power, and signatures are
//! [`DuplicateVoteEvidence::verify`], matching `evidence.VerifyDuplicateVote`.
//! A light-client attack is [`LightClientAttackEvidence::verify`], matching
//! `evidence.VerifyLightClientAttack`.

use prost::Message;

use eld_tendermint_crypto::{hash_from_byte_slices, sum};

use crate::light::{LightBlock, SignedHeader};
use crate::{Error, Hash, Header, Time, Validator, ValidatorSet, Vote};

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

/// The two evidence sums in `tendermint.types.Evidence`.
///
/// Both sums stay inline. A light block is larger than a duplicate vote, and boxing
/// one of them would make every pool row an extra allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Evidence {
    Duplicate(DuplicateVoteEvidence),
    Light(LightClientAttackEvidence),
}

impl From<DuplicateVoteEvidence> for Evidence {
    fn from(evidence: DuplicateVoteEvidence) -> Self {
        Self::Duplicate(evidence)
    }
}

impl From<LightClientAttackEvidence> for Evidence {
    fn from(evidence: LightClientAttackEvidence) -> Self {
        Self::Light(evidence)
    }
}

impl Evidence {
    /// Height used as the pool key. A light-client attack uses `common_height`.
    #[must_use]
    pub fn height(&self) -> i64 {
        match self {
            Self::Duplicate(evidence) => evidence.vote_a.height,
            Self::Light(evidence) => evidence.common_height,
        }
    }

    /// Proto bytes hashed into an [`EvidenceList`]. Not the oneof wrapper.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Self::Duplicate(evidence) => evidence.bytes(),
            Self::Light(evidence) => evidence.bytes(),
        }
    }

    #[must_use]
    pub fn hash(&self) -> Hash {
        match self {
            Self::Duplicate(evidence) => evidence.hash(),
            Self::Light(evidence) => evidence.hash(),
        }
    }

    /// # Errors
    ///
    /// Returns the item's `ValidateBasic` error.
    pub fn validate_basic(&self) -> Result<(), Error> {
        match self {
            Self::Duplicate(evidence) => evidence.validate_basic(),
            Self::Light(evidence) => evidence.validate_basic(),
        }
    }

    #[must_use]
    pub fn to_evidence_proto(&self) -> eld_tendermint_proto::types::Evidence {
        match self {
            Self::Duplicate(evidence) => evidence.to_evidence_proto(),
            Self::Light(evidence) => evidence.to_evidence_proto(),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::UnsupportedEvidence`] when the sum is missing, otherwise the
    /// item's decode error.
    pub fn try_from_evidence_proto(
        proto: &eld_tendermint_proto::types::Evidence,
    ) -> Result<Self, Error> {
        match &proto.sum {
            Some(eld_tendermint_proto::types::evidence::Sum::DuplicateVoteEvidence(evidence)) => {
                Ok(Self::Duplicate(DuplicateVoteEvidence::try_from_proto(
                    evidence,
                )?))
            }
            Some(eld_tendermint_proto::types::evidence::Sum::LightClientAttackEvidence(
                evidence,
            )) => Ok(Self::Light(LightClientAttackEvidence::try_from_proto(
                evidence,
            )?)),
            None => Err(Error::UnsupportedEvidence),
        }
    }
}

/// `types.LightClientAttackEvidence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightClientAttackEvidence {
    pub conflicting_block: LightBlock,
    pub common_height: i64,
    pub byzantine_validators: Vec<Validator>,
    pub total_voting_power: i64,
    pub timestamp: Time,
}

impl LightClientAttackEvidence {
    /// `LightClientAttackEvidence.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns a missing-block, power, or height error, or the conflicting block's
    /// [`LightBlock::validate_basic`] error.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.total_voting_power <= 0 {
            return Err(Error::NonPositiveTotalVotingPower);
        }
        if self.common_height <= 0 {
            return Err(Error::NonPositiveCommonHeight);
        }
        if self.common_height > self.conflicting_block.signed_header.header.height {
            return Err(Error::CommonHeightAhead);
        }
        let chain_id = self
            .conflicting_block
            .signed_header
            .header
            .chain_id
            .as_str();
        self.conflicting_block.validate_basic(chain_id)
    }

    /// True when a deterministic header field differs. That is a lunatic attack.
    #[must_use]
    pub fn conflicting_header_is_invalid(&self, trusted: &Header) -> bool {
        let header = &self.conflicting_block.signed_header.header;
        trusted.validators_hash != header.validators_hash
            || trusted.next_validators_hash != header.next_validators_hash
            || trusted.consensus_hash != header.consensus_hash
            || trusted.app_hash != header.app_hash
            || trusted.last_results_hash != header.last_results_hash
    }

    /// `GetByzantineValidators`. Lunatic signers come from `common_vals`. Equivocation
    /// signers voted in both same-round commits. Amnesia returns an empty list.
    /// The result is ordered by voting power descending, then address.
    #[must_use]
    pub fn get_byzantine_validators(
        &self,
        common_vals: &ValidatorSet,
        trusted: &SignedHeader,
    ) -> Vec<Validator> {
        let mut validators = Vec::new();
        let conflicting_commit = &self.conflicting_block.signed_header.commit;
        if self.conflicting_header_is_invalid(&trusted.header) {
            for sig in &conflicting_commit.signatures {
                if !sig.for_block() {
                    continue;
                }
                if let Some(validator) = common_vals
                    .validators()
                    .iter()
                    .find(|validator| validator.address == sig.validator_address)
                {
                    validators.push(validator.clone());
                }
            }
        } else if trusted.commit.round == conflicting_commit.round {
            for (index, sig) in conflicting_commit.signatures.iter().enumerate() {
                if sig.is_absent() {
                    continue;
                }
                let Some(other) = trusted.commit.signatures.get(index) else {
                    continue;
                };
                if other.is_absent() {
                    continue;
                }
                if let Some(validator) = self
                    .conflicting_block
                    .validator_set
                    .validators()
                    .iter()
                    .find(|validator| validator.address == sig.validator_address)
                {
                    validators.push(validator.clone());
                }
            }
        } else {
            return validators;
        }
        validators.sort_by(|left, right| {
            right
                .voting_power
                .cmp(&left.voting_power)
                .then_with(|| left.address.cmp(&right.address))
        });
        validators
    }

    /// `VerifyLightClientAttack`.
    ///
    /// `now` and the trust period are unused in the Go function, so expiry stays with
    /// the caller. `common` and `trusted` are the same header when the heights match.
    ///
    /// # Errors
    ///
    /// Returns a commit, power, hash, time, or byzantine-validator error.
    pub fn verify(
        &self,
        common: &SignedHeader,
        trusted: &SignedHeader,
        common_vals: &ValidatorSet,
    ) -> Result<(), Error> {
        let conflicting = &self.conflicting_block;
        if common.header.height != conflicting.signed_header.header.height {
            common_vals.verify_commit_light_trusting(
                trusted.header.chain_id.as_str(),
                &conflicting.signed_header.commit,
            )?;
        } else if self.conflicting_header_is_invalid(&trusted.header) {
            return Err(Error::ConflictingHeaderDerived);
        }
        conflicting.validator_set.verify_commit_light(
            trusted.header.chain_id.as_str(),
            &conflicting.signed_header.commit.block_id,
            conflicting.signed_header.header.height,
            &conflicting.signed_header.commit,
        )?;
        if self.total_voting_power != common_vals.total_voting_power() {
            return Err(Error::EvidenceTotalPowerMismatch);
        }
        if conflicting.signed_header.header.height > trusted.header.height {
            if time_after(conflicting.signed_header.header.time, trusted.header.time) {
                return Err(Error::ConflictingTimeOrder);
            }
        } else if conflicting.signed_header.header.hash() == trusted.header.hash() {
            return Err(Error::TrustedHashMatches);
        }
        self.validate_abci(common_vals, trusted)
    }

    /// `LightClientAttackEvidence.Bytes`: the marshaled evidence proto, not the oneof.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        self.to_proto()
            .map(|proto| proto.encode_to_vec())
            .unwrap_or_default()
    }

    /// `LightClientAttackEvidence.Hash`. Copies 31 bytes of the conflicting header hash,
    /// leaves the next byte zero, then appends the varint of `common_height`.
    #[must_use]
    pub fn hash(&self) -> Hash {
        let header_hash = self
            .conflicting_block
            .signed_header
            .header
            .hash()
            .map(|hash| *hash.as_bytes())
            .unwrap_or([0; crate::HASH_SIZE]);
        let varint = put_varint(self.common_height);
        let mut body = vec![0_u8; crate::HASH_SIZE + varint.len()];
        body[..crate::HASH_SIZE - 1].copy_from_slice(&header_hash[..crate::HASH_SIZE - 1]);
        body[crate::HASH_SIZE..].copy_from_slice(&varint);
        Hash::from_array(sum(&body))
    }

    /// # Errors
    ///
    /// Returns a light-block proto error.
    pub fn to_proto(
        &self,
    ) -> Result<eld_tendermint_proto::types::LightClientAttackEvidence, Error> {
        let mut byzantine_validators = Vec::with_capacity(self.byzantine_validators.len());
        for validator in &self.byzantine_validators {
            byzantine_validators.push(validator.to_proto()?);
        }
        Ok(eld_tendermint_proto::types::LightClientAttackEvidence {
            conflicting_block: Some(self.conflicting_block.to_proto()?),
            common_height: self.common_height,
            byzantine_validators,
            total_voting_power: self.total_voting_power,
            timestamp: Some(self.timestamp.to_prost()),
        })
    }

    /// `EvidenceToProto` for the light-client sum.
    #[must_use]
    pub fn to_evidence_proto(&self) -> eld_tendermint_proto::types::Evidence {
        eld_tendermint_proto::types::Evidence {
            sum: self.to_proto().ok().map(|evidence| {
                eld_tendermint_proto::types::evidence::Sum::LightClientAttackEvidence(evidence)
            }),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::MissingConflictingBlock`] or [`Self::validate_basic`].
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::LightClientAttackEvidence,
    ) -> Result<Self, Error> {
        let Some(conflicting_block) = proto.conflicting_block.as_ref() else {
            return Err(Error::MissingConflictingBlock);
        };
        let evidence = Self {
            conflicting_block: LightBlock::try_from_proto(conflicting_block)?,
            common_height: proto.common_height,
            byzantine_validators: proto
                .byzantine_validators
                .iter()
                .map(Validator::try_from_proto)
                .collect::<Result<Vec<_>, _>>()?,
            total_voting_power: proto.total_voting_power,
            timestamp: Time::from_prost(proto.timestamp.as_ref()),
        };
        evidence.validate_basic()?;
        Ok(evidence)
    }

    fn validate_abci(
        &self,
        common_vals: &ValidatorSet,
        trusted: &SignedHeader,
    ) -> Result<(), Error> {
        if self.total_voting_power != common_vals.total_voting_power() {
            return Err(Error::EvidenceTotalPowerMismatch);
        }
        let validators = self.get_byzantine_validators(common_vals, trusted);
        if validators.len() != self.byzantine_validators.len() {
            return Err(Error::ByzantineValidatorMismatch);
        }
        for (expected, got) in validators.iter().zip(&self.byzantine_validators) {
            if expected.address != got.address || expected.voting_power != got.voting_power {
                return Err(Error::ByzantineValidatorMismatch);
            }
        }
        Ok(())
    }
}

/// `types.EvidenceList`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvidenceList {
    pub evidence: Vec<Evidence>,
}

impl EvidenceList {
    #[must_use]
    pub fn new(evidence: Vec<Evidence>) -> Self {
        Self { evidence }
    }

    /// `EvidenceList.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns the first item's `ValidateBasic` error.
    pub fn validate_basic(&self) -> Result<(), Error> {
        for evidence in &self.evidence {
            evidence.validate_basic()?;
        }
        Ok(())
    }

    /// `EvidenceList.Hash`: merkle root of each item's evidence bytes.
    ///
    /// An empty list is the empty Merkle root.
    #[must_use]
    pub fn hash(&self) -> Hash {
        let bytes: Vec<Vec<u8>> = self.evidence.iter().map(Evidence::bytes).collect();
        Hash::from_array(hash_from_byte_slices(&bytes))
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::EvidenceList {
        eld_tendermint_proto::types::EvidenceList {
            evidence: self
                .evidence
                .iter()
                .map(Evidence::to_evidence_proto)
                .collect(),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::UnsupportedEvidence`] for a missing sum, or an item error.
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::EvidenceList,
    ) -> Result<Self, Error> {
        let mut evidence = Vec::with_capacity(proto.evidence.len());
        for item in &proto.evidence {
            evidence.push(Evidence::try_from_evidence_proto(item)?);
        }
        Ok(Self { evidence })
    }
}

fn time_after(left: Time, right: Time) -> bool {
    (left.unix_seconds(), left.nanos()) > (right.unix_seconds(), right.nanos())
}

/// `binary.PutVarint`.
fn put_varint(value: i64) -> Vec<u8> {
    let mut unsigned = (value as u64) << 1;
    if value < 0 {
        unsigned = !unsigned;
    }
    let mut out = Vec::new();
    loop {
        let mut byte = (unsigned & 0x7f) as u8;
        unsigned >>= 7;
        if unsigned != 0 {
            byte |= 0x80;
            out.push(byte);
        } else {
            out.push(byte);
            break;
        }
    }
    out
}
