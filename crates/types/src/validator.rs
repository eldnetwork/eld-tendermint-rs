//! `types.Validator` and `ValidatorSet`.
//!
//! `IncrementProposerPriority` recenters priorities, then adds each validator's voting
//! power and subtracts the total from the proposer. Go panics on an empty set or a
//! non-positive `times`; those cases return [`Error`].

use std::cmp::Ordering;
use std::collections::BTreeSet;

use prost::Message;

use eld_tendermint_crypto::{PubKey, hash_from_byte_slices, pub_key_from_proto, pub_key_to_proto};

use crate::{ADDRESS_SIZE, Error, Hash, MAX_TOTAL_VOTING_POWER};

/// `types.PriorityWindowSizeFactor`. The priority gap is capped at twice the total power.
const PRIORITY_WINDOW_SIZE_FACTOR: i64 = 2;

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

    /// `Validator.ToProto`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingPubKey`] when `pub_key` is absent.
    pub fn to_proto(&self) -> Result<eld_tendermint_proto::types::Validator, Error> {
        let pub_key = self.pub_key.as_ref().ok_or(Error::MissingPubKey)?;
        Ok(eld_tendermint_proto::types::Validator {
            address: self.address.clone(),
            pub_key: Some(pub_key_to_proto(pub_key)),
            voting_power: self.voting_power,
            proposer_priority: self.proposer_priority,
        })
    }

    /// `ValidatorFromProto`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingPubKey`] when the key is missing, or [`Error::PubKey`]
    /// when the key bytes are not Ed25519.
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Validator) -> Result<Self, Error> {
        let pub_key = proto.pub_key.as_ref().ok_or(Error::MissingPubKey)?;
        let pub_key = pub_key_from_proto(pub_key).map_err(Error::PubKey)?;
        Ok(Self {
            address: proto.address.clone(),
            pub_key: Some(pub_key),
            voting_power: proto.voting_power,
            proposer_priority: proto.proposer_priority,
        })
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

/// `types.ValidatorSet`.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ValidatorSet {
    validators: Vec<Validator>,
    proposer_index: Option<usize>,
    total_voting_power: i64,
}

impl ValidatorSet {
    /// `NewValidatorSet`.
    ///
    /// Sorts by voting power descending, then address ascending. Each new validator's
    /// priority starts at `-(total + (total >> 3))`, then the set is rescaled, shifted
    /// so the average is near zero, and incremented once.
    ///
    /// Pubkey presence and the 20-byte address check stay on [`Validator::validate_basic`].
    /// This constructor accepts the short addresses used by the Go proposer fixtures.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyValidatorSet`], [`Error::NegativeVotingPower`],
    /// [`Error::ZeroVotingPower`], [`Error::DuplicateValidator`], or
    /// [`Error::VotingPowerTooHigh`].
    pub fn new(mut validators: Vec<Validator>) -> Result<Self, Error> {
        if validators.is_empty() {
            return Err(Error::EmptyValidatorSet);
        }
        let mut seen = BTreeSet::new();
        let mut total = 0i64;
        for validator in &validators {
            if validator.voting_power < 0 {
                return Err(Error::NegativeVotingPower);
            }
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
        // `computeNewPriorities` for validators that are not already in the set.
        let initial = -(total + (total >> 3));
        for validator in &mut validators {
            validator.proposer_priority = initial;
        }
        let mut set = Self {
            validators,
            proposer_index: None,
            total_voting_power: total,
        };
        let diff_max = PRIORITY_WINDOW_SIZE_FACTOR * total;
        set.rescale_priorities(diff_max);
        set.shift_by_avg_proposer_priority();
        set.validators.sort_by(validator_power_order);
        set.increment_proposer_priority(1)?;
        Ok(set)
    }

    /// `ValidatorSet.Copy`. A deep copy: incrementing the result does not change `self`.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// `ValidatorSet.IncrementProposerPriority`.
    ///
    /// Rescales, subtracts the average priority, then adds voting power to every
    /// validator and subtracts the total from the winner, `times` times.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyValidatorSet`] or [`Error::NonPositiveTimes`]. Go panics
    /// on both (`"empty validator set"` and `"Cannot call IncrementProposerPriority
    /// with non-positive times"`).
    pub fn increment_proposer_priority(&mut self, times: i32) -> Result<(), Error> {
        if self.validators.is_empty() {
            return Err(Error::EmptyValidatorSet);
        }
        if times <= 0 {
            return Err(Error::NonPositiveTimes);
        }
        let diff_max = PRIORITY_WINDOW_SIZE_FACTOR * self.total_voting_power;
        self.rescale_priorities(diff_max);
        self.shift_by_avg_proposer_priority();
        let mut proposer = 0;
        for _ in 0..times {
            proposer = self.increment_one();
        }
        self.proposer_index = Some(proposer);
        Ok(())
    }

    /// `RescalePriorities`. No change when `diff_max <= 0` or the spread already fits.
    fn rescale_priorities(&mut self, diff_max: i64) {
        if diff_max <= 0 {
            return;
        }
        let diff = max_min_priority_diff(&self.validators);
        let diff_max = i128::from(diff_max);
        if diff > diff_max {
            let ratio = (diff + diff_max - 1) / diff_max;
            let ratio = i64::try_from(ratio).unwrap_or(i64::MAX);
            for validator in &mut self.validators {
                validator.proposer_priority /= ratio;
            }
        }
    }

    /// `shiftByAvgProposerPriority`.
    fn shift_by_avg_proposer_priority(&mut self) {
        let average = self.average_proposer_priority();
        for validator in &mut self.validators {
            validator.proposer_priority = safe_sub_clip(validator.proposer_priority, average);
        }
    }

    /// `computeAvgProposerPriority`. The sum is `i128` so it matches Go's `big.Int` and
    /// does not overflow. Division truncates toward zero.
    ///
    /// # Panics
    ///
    /// Panics if the average does not fit in `i64`. Go panics with
    /// `"Cannot represent avg ProposerPriority as an int64"`. That does not happen while
    /// each priority fits in `i64`.
    fn average_proposer_priority(&self) -> i64 {
        let count = i128::try_from(self.validators.len()).expect("validator count fits in i128");
        let sum = self.validators.iter().fold(0i128, |sum, validator| {
            sum + i128::from(validator.proposer_priority)
        });
        let average = sum / count;
        i64::try_from(average).expect("avg proposer priority fits in i64")
    }

    /// One step of `incrementProposerPriority`: add voting power, then subtract the total
    /// from the validator with the highest priority.
    fn increment_one(&mut self) -> usize {
        for validator in &mut self.validators {
            validator.proposer_priority =
                safe_add_clip(validator.proposer_priority, validator.voting_power);
        }
        let winner = pick_proposer(&self.validators);
        let total = self.total_voting_power;
        let priority = self.validators[winner].proposer_priority;
        self.validators[winner].proposer_priority = safe_sub_clip(priority, total);
        winner
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

    /// `ValidatorSet.UpdateWithChangeSet`. Deletes (power 0) are allowed.
    ///
    /// A power change keeps the validator's proposer priority. A new validator's
    /// priority is `-(updated_total + (updated_total >> 3))` against the total
    /// after updates and before removals. The set is then rescaled, centered,
    /// and sorted by power. The caller's `changes` are not modified.
    ///
    /// `proposer` is cleared. Go leaves a stale proposer until the next
    /// `IncrementProposerPriority`. Callers that need one increment afterwards.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateValidator`], [`Error::NegativeVotingPower`],
    /// [`Error::ValidatorVotingPowerTooHigh`], [`Error::EmptyValidatorSet`] when
    /// the result would be empty, [`Error::ValidatorNotInSet`] when a removal
    /// is not in the set, or [`Error::VotingPowerTooHigh`].
    pub fn update_with_change_set(&mut self, changes: &[Validator]) -> Result<(), Error> {
        if changes.is_empty() {
            return Ok(());
        }
        let (updates, deletes) = process_changes(changes)?;
        if new_validator_count(&updates, self) == 0 && self.validators.len() == deletes.len() {
            return Err(Error::EmptyValidatorSet);
        }
        let removed_power = verify_removals(&deletes, self)?;
        let updated_total = verify_updates(&updates, self, removed_power)?;
        let mut updates = updates;
        compute_new_priorities(&mut updates, self, updated_total);
        self.apply_updates(updates);
        self.apply_removals(deletes);
        self.recompute_total_voting_power()?;
        let diff_max = PRIORITY_WINDOW_SIZE_FACTOR * self.total_voting_power;
        self.rescale_priorities(diff_max);
        self.shift_by_avg_proposer_priority();
        self.validators.sort_by(validator_power_order);
        self.proposer_index = None;
        Ok(())
    }

    fn get_by_address(&self, address: &[u8]) -> Option<&Validator> {
        self.validators
            .iter()
            .find(|validator| validator.address == address)
    }

    fn apply_updates(&mut self, mut updates: Vec<Validator>) {
        let mut existing = std::mem::take(&mut self.validators);
        existing.sort_by(|left, right| left.address.cmp(&right.address));
        updates.sort_by(|left, right| left.address.cmp(&right.address));
        let mut merged = Vec::with_capacity(existing.len() + updates.len());
        let mut existing = existing.into_iter().peekable();
        let mut updates = updates.into_iter().peekable();
        loop {
            match (existing.peek(), updates.peek()) {
                (Some(current), Some(update)) => match current.address.cmp(&update.address) {
                    Ordering::Less => merged.push(existing.next().expect("peeked")),
                    Ordering::Equal => {
                        existing.next();
                        merged.push(updates.next().expect("peeked"));
                    }
                    Ordering::Greater => merged.push(updates.next().expect("peeked")),
                },
                (Some(_), None) => {
                    merged.extend(existing);
                    break;
                }
                (None, Some(_)) => {
                    merged.extend(updates);
                    break;
                }
                (None, None) => break,
            }
        }
        self.validators = merged;
    }

    fn apply_removals(&mut self, deletes: Vec<Validator>) {
        if deletes.is_empty() {
            return;
        }
        let existing = std::mem::take(&mut self.validators);
        let mut merged = Vec::with_capacity(existing.len() - deletes.len());
        let mut deletes = deletes.into_iter().peekable();
        for validator in existing {
            if deletes
                .peek()
                .is_some_and(|delete| delete.address == validator.address)
            {
                deletes.next();
            } else {
                merged.push(validator);
            }
        }
        self.validators = merged;
    }

    fn recompute_total_voting_power(&mut self) -> Result<(), Error> {
        let mut sum = 0i64;
        for validator in &self.validators {
            sum = safe_add_clip(sum, validator.voting_power);
            if sum > MAX_TOTAL_VOTING_POWER {
                return Err(Error::VotingPowerTooHigh);
            }
        }
        self.total_voting_power = sum;
        Ok(())
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

    /// `ValidatorSet.ToProto`. `total_voting_power` is 0 on the wire. Go ignores a
    /// peer-supplied total and recomputes it in `ValidatorSetFromProto`.
    ///
    /// An empty set encodes as an empty message. A non-empty set includes its proposer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingProposer`] when a non-empty set has no proposer, or a
    /// validator proto error.
    pub fn to_proto(&self) -> Result<eld_tendermint_proto::types::ValidatorSet, Error> {
        if self.validators.is_empty() {
            return Ok(eld_tendermint_proto::types::ValidatorSet::default());
        }
        let mut validators = Vec::with_capacity(self.validators.len());
        for validator in &self.validators {
            validators.push(validator.to_proto()?);
        }
        let proposer = self.proposer().ok_or(Error::MissingProposer)?.to_proto()?;
        Ok(eld_tendermint_proto::types::ValidatorSet {
            validators,
            proposer: Some(proposer),
            total_voting_power: 0,
        })
    }

    /// `ValidatorSetFromProto`. Total power is recomputed. An empty message is an empty set.
    ///
    /// # Errors
    ///
    /// Returns a validator error, [`Error::MissingProposer`], or [`Error::VotingPowerTooHigh`].
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::ValidatorSet,
    ) -> Result<Self, Error> {
        if proto.validators.is_empty() {
            return Ok(Self::default());
        }
        let mut validators = Vec::with_capacity(proto.validators.len());
        for validator in &proto.validators {
            validators.push(Validator::try_from_proto(validator)?);
        }
        let proposer = proto
            .proposer
            .as_ref()
            .ok_or(Error::MissingProposer)
            .and_then(Validator::try_from_proto)?;
        let proposer_index = validators
            .iter()
            .position(|validator| validator.address == proposer.address)
            .ok_or(Error::MissingProposer)?;
        let mut total_voting_power = 0i64;
        for validator in &validators {
            total_voting_power = total_voting_power
                .checked_add(validator.voting_power)
                .filter(|sum| *sum <= MAX_TOTAL_VOTING_POWER)
                .ok_or(Error::VotingPowerTooHigh)?;
        }
        let set = Self {
            validators,
            proposer_index: Some(proposer_index),
            total_voting_power,
        };
        set.validate_basic()?;
        Ok(set)
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

/// `safeAddClip`. Overflow clips to `i64::MAX` when `b >= 0`, otherwise `i64::MIN`.
fn safe_add_clip(a: i64, b: i64) -> i64 {
    match a.checked_add(b) {
        Some(sum) => sum,
        None if b < 0 => i64::MIN,
        None => i64::MAX,
    }
}

/// `safeSubClip`. Overflow clips to `i64::MIN` when `b > 0`, otherwise `i64::MAX`.
fn safe_sub_clip(a: i64, b: i64) -> i64 {
    match a.checked_sub(b) {
        Some(difference) => difference,
        None if b > 0 => i64::MIN,
        None => i64::MAX,
    }
}

fn process_changes(changes: &[Validator]) -> Result<(Vec<Validator>, Vec<Validator>), Error> {
    let mut changes = changes.to_vec();
    changes.sort_by(|left, right| left.address.cmp(&right.address));
    let mut updates = Vec::new();
    let mut removals = Vec::new();
    let mut previous = Vec::new();
    for change in changes {
        if change.address == previous {
            return Err(Error::DuplicateValidator);
        }
        if change.voting_power < 0 {
            return Err(Error::NegativeVotingPower);
        }
        if change.voting_power > MAX_TOTAL_VOTING_POWER {
            return Err(Error::ValidatorVotingPowerTooHigh {
                got: change.voting_power,
            });
        }
        previous = change.address.clone();
        if change.voting_power == 0 {
            removals.push(change);
        } else {
            updates.push(change);
        }
    }
    Ok((updates, removals))
}

fn new_validator_count(updates: &[Validator], set: &ValidatorSet) -> usize {
    updates
        .iter()
        .filter(|update| set.get_by_address(&update.address).is_none())
        .count()
}

fn verify_removals(deletes: &[Validator], set: &ValidatorSet) -> Result<i64, Error> {
    let mut removed = 0i64;
    for update in deletes {
        let Some(validator) = set.get_by_address(&update.address) else {
            return Err(Error::ValidatorNotInSet);
        };
        removed = safe_add_clip(removed, validator.voting_power);
    }
    Ok(removed)
}

fn power_delta(update: &Validator, set: &ValidatorSet) -> i64 {
    match set.get_by_address(&update.address) {
        Some(validator) => update.voting_power.wrapping_sub(validator.voting_power),
        None => update.voting_power,
    }
}

fn verify_updates(
    updates: &[Validator],
    set: &ValidatorSet,
    removed_power: i64,
) -> Result<i64, Error> {
    let mut ordered = updates.to_vec();
    ordered.sort_by_key(|update| power_delta(update, set));
    let mut total = set.total_voting_power.wrapping_sub(removed_power);
    for update in &ordered {
        total = total.wrapping_add(power_delta(update, set));
        if total > MAX_TOTAL_VOTING_POWER {
            return Err(Error::VotingPowerTooHigh);
        }
    }
    Ok(total.wrapping_add(removed_power))
}

fn compute_new_priorities(updates: &mut [Validator], set: &ValidatorSet, updated_total: i64) {
    for update in updates {
        update.proposer_priority = match set.get_by_address(&update.address) {
            Some(validator) => validator.proposer_priority,
            None => -(updated_total + (updated_total >> 3)),
        };
    }
}

/// `computeMaxMinPriorityDiff`. The spread is computed in `i128` so `max - min` cannot wrap.
fn max_min_priority_diff(validators: &[Validator]) -> i128 {
    let mut max = i64::MIN;
    let mut min = i64::MAX;
    for validator in validators {
        if validator.proposer_priority < min {
            min = validator.proposer_priority;
        }
        if validator.proposer_priority > max {
            max = validator.proposer_priority;
        }
    }
    i128::from(max) - i128::from(min)
}
