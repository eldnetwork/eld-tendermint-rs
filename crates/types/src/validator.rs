//! `types.Validator` and `ValidatorSet`.
//!
//! Proposer priority is stored and used only to pick the proposer when every priority
//! is already set (all zeros selects the lowest address). `IncrementProposerPriority`
//! is not implemented.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use prost::Message;

use eld_tendermint_crypto::{PubKey, hash_from_byte_slices, pub_key_to_proto};

use crate::{ADDRESS_SIZE, Error, Hash, MAX_TOTAL_VOTING_POWER};

/// `types.Validator`. `validate_basic` does not check that `address` matches `pub_key`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validator {
    pub address: Vec<u8>,
    pub pub_key: Option<PubKey>,
    pub voting_power: i64,
    pub proposer_priority: i64,
}

impl Validator {
    /// `NewValidator`: address is the pubkey address and proposer priority is 0.
    #[must_use]
    pub fn new(pub_key: PubKey, voting_power: i64) -> Self {
        Self {
            address: pub_key.address().to_vec(),
            pub_key: Some(pub_key),
            voting_power,
            proposer_priority: 0,
        }
    }

    /// `Validator.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when the key is missing, voting power is negative, or the address
    /// is not 20 bytes.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.pub_key.is_none() {
            return Err(Error::MissingPubKey);
        }
        if self.voting_power < 0 {
            return Err(Error::NegativeVotingPower);
        }
        if self.address.len() != ADDRESS_SIZE {
            return Err(Error::InvalidAddressLength {
                len: self.address.len(),
            });
        }
        Ok(())
    }

    /// Protobuf `SimpleValidator` (pubkey and voting power). Address and priority are excluded.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingPubKey`] when `pub_key` is absent. Go panics in that case.
    pub fn bytes(&self) -> Result<Vec<u8>, Error> {
        let pub_key = self.pub_key.as_ref().ok_or(Error::MissingPubKey)?;
        Ok(eld_tendermint_proto::types::SimpleValidator {
            pub_key: Some(pub_key_to_proto(pub_key)),
            voting_power: self.voting_power,
        }
        .encode_to_vec())
    }
}

/// `types.ValidatorSet`, without proposer-priority rotation.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ValidatorSet {
    validators: Vec<Validator>,
    proposer_index: Option<usize>,
    total_voting_power: i64,
}

impl ValidatorSet {
    /// Sorts by voting power descending, then address ascending, and sums voting power.
    ///
    /// Rejects an empty set, duplicate addresses, and voting power of 0, matching
    /// `NewValidatorSet` / `updateWithChangeSet`. Does not run `IncrementProposerPriority`.
    ///
    /// # Errors
    ///
    /// Returns a validation error from a validator, or [`Error::EmptyValidatorSet`],
    /// [`Error::DuplicateValidator`], [`Error::ZeroVotingPower`], or [`Error::VotingPowerTooHigh`].
    pub fn new(mut validators: Vec<Validator>) -> Result<Self, Error> {
        if validators.is_empty() {
            return Err(Error::EmptyValidatorSet);
        }
        let mut seen = BTreeSet::new();
        let mut total = 0i64;
        for validator in &validators {
            validator.validate_basic()?;
            if validator.voting_power == 0 {
                return Err(Error::ZeroVotingPower);
            }
            if !seen.insert(validator.address.clone()) {
                return Err(Error::DuplicateValidator);
            }
            total = total
                .checked_add(validator.voting_power)
                .filter(|sum| *sum <= MAX_TOTAL_VOTING_POWER)
                .ok_or(Error::VotingPowerTooHigh)?;
        }
        validators.sort_by(validator_power_order);
        let proposer_index = pick_proposer(&validators);
        Ok(Self {
            validators,
            proposer_index: Some(proposer_index),
            total_voting_power: total,
        })
    }

    #[must_use]
    pub fn validators(&self) -> &[Validator] {
        &self.validators
    }

    #[must_use]
    pub fn proposer(&self) -> Option<&Validator> {
        self.proposer_index.map(|index| &self.validators[index])
    }

    #[must_use]
    pub fn total_voting_power(&self) -> i64 {
        self.total_voting_power
    }

    /// `ValidatorSet.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when the set or proposer is missing, or a validator fails
    /// [`Validator::validate_basic`].
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.validators.is_empty() {
            return Err(Error::EmptyValidatorSet);
        }
        for validator in &self.validators {
            validator.validate_basic()?;
        }
        let index = self.proposer_index.ok_or(Error::MissingProposer)?;
        self.validators
            .get(index)
            .ok_or(Error::MissingProposer)?
            .validate_basic()
    }

    /// Merkle root of each [`Validator::bytes`] in the set's current order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingPubKey`] when a validator has no key.
    pub fn hash(&self) -> Result<Hash, Error> {
        let mut leaves = Vec::with_capacity(self.validators.len());
        for validator in &self.validators {
            leaves.push(validator.bytes()?);
        }
        Ok(Hash::from_array(hash_from_byte_slices(&leaves)))
    }
}

fn validator_power_order(left: &Validator, right: &Validator) -> Ordering {
    right
        .voting_power
        .cmp(&left.voting_power)
        .then_with(|| left.address.cmp(&right.address))
}

/// Lowest address wins when priorities are equal. Identical addresses are skipped.
fn pick_proposer(validators: &[Validator]) -> usize {
    let mut best = 0;
    for index in 1..validators.len() {
        if validators[index].address == validators[best].address {
            continue;
        }
        if wins_priority(&validators[index], &validators[best]) {
            best = index;
        }
    }
    best
}

fn wins_priority(candidate: &Validator, current: &Validator) -> bool {
    match candidate.proposer_priority.cmp(&current.proposer_priority) {
        Ordering::Greater => true,
        Ordering::Less => false,
        Ordering::Equal => candidate.address < current.address,
    }
}
