//! Pending and committed evidence.
//!
//! Keys match `evidence/pool.go`: `0x01` is pending and `0x00` is committed.
//! The suffix is the evidence height, 16 uppercase hex digits, then `/`, then the
//! uppercase hex of the evidence hash. A light-client attack uses `common_height`.

use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_state::State;
use eld_tendermint_store::Db;
use eld_tendermint_types::{
    DuplicateVoteEvidence, Evidence, EvidenceList, LightClientAttackEvidence, SignedHeader,
    ValidatorSet,
};
use prost::Message;

use crate::Error;

const PENDING: u8 = 0x01;
const COMMITTED: u8 = 0x00;

struct Sets {
    chain_id: String,
    validators: eld_tendermint_types::ValidatorSet,
    last_validators: eld_tendermint_types::ValidatorSet,
}

struct Inner<D: Db> {
    db: D,
    sets: Sets,
}

/// What the proposer reads. [`Pool`] implements this.
pub trait ProposalEvidence: Send + Sync {
    /// Uncommitted evidence that fits in `max_bytes` of an `EvidenceList`.
    fn pending(&self, max_bytes: i64) -> Vec<Evidence>;

    /// Move `evidence` from pending to committed so it is not proposed again.
    fn mark_committed(&self, evidence: &EvidenceList);

    /// Replace the validator sets used by the next [`Pool::add`].
    fn update_state(&self, state: &State);
}

/// Evidence stored in its own database, separate from the block store.
pub struct Pool<D: Db> {
    inner: Arc<Mutex<Inner<D>>>,
}

impl<D: Db> Clone for Pool<D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D: Db + 'static> Pool<D> {
    /// Share this pool with a consensus node.
    #[must_use]
    pub fn for_proposal(self) -> Arc<dyn ProposalEvidence> {
        Arc::new(self)
    }
}

impl<D: Db> Pool<D> {
    /// Empty pool over `db`, verified against `state`'s current and last sets.
    #[must_use]
    pub fn new(db: D, state: &State) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                db,
                sets: sets_from(state),
            })),
        }
    }

    /// Verify and store `evidence`.
    ///
    /// An evidence hash that is already pending or committed is ignored.
    /// `validate_basic` runs first. `verify` tries the current validator set,
    /// then the last set when the signer is absent from the current one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the evidence fails those checks, and
    /// [`Error::Db`] when the pending row cannot be written. Nothing is written
    /// on an invalid vote.
    pub fn add(&self, evidence: DuplicateVoteEvidence) -> Result<(), Error> {
        let wrapped = Evidence::Duplicate(evidence);
        let inner = lock(&self.inner);
        if row_present(&inner.db, &evidence_key(PENDING, &wrapped))?
            || row_present(&inner.db, &evidence_key(COMMITTED, &wrapped))?
        {
            return Ok(());
        }
        let Evidence::Duplicate(evidence) = &wrapped else {
            return Ok(());
        };
        evidence.validate_basic().map_err(Error::Invalid)?;
        verify_against_sets(&inner.sets, evidence)?;
        let bytes = wrapped.to_evidence_proto().encode_to_vec();
        inner
            .db
            .set_sync(&evidence_key(PENDING, &wrapped), &bytes)
            .map_err(|err| Error::Db(err.to_string()))
    }

    /// Verify a light-client attack and store it.
    ///
    /// The caller supplies the common and trusted signed headers and the common
    /// validator set. This pool has no block store. An evidence hash that is already
    /// pending or committed is ignored. A failed check writes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when `validate_basic` or `verify` fails, and
    /// [`Error::Db`] when the pending row cannot be written.
    pub fn add_light(
        &self,
        evidence: LightClientAttackEvidence,
        common: &SignedHeader,
        trusted: &SignedHeader,
        common_vals: &ValidatorSet,
    ) -> Result<(), Error> {
        let wrapped = Evidence::Light(evidence);
        let inner = lock(&self.inner);
        if row_present(&inner.db, &evidence_key(PENDING, &wrapped))?
            || row_present(&inner.db, &evidence_key(COMMITTED, &wrapped))?
        {
            return Ok(());
        }
        let Evidence::Light(evidence) = &wrapped else {
            return Err(Error::Invalid(
                eld_tendermint_types::Error::UnsupportedEvidence,
            ));
        };
        evidence.validate_basic().map_err(Error::Invalid)?;
        evidence
            .verify(common, trusted, common_vals)
            .map_err(Error::Invalid)?;
        let bytes = wrapped.to_evidence_proto().encode_to_vec();
        inner
            .db
            .set_sync(&evidence_key(PENDING, &wrapped), &bytes)
            .map_err(|err| Error::Db(err.to_string()))
    }

    /// A gossiped light-client attack has no trusted header here, so it is not stored.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] with [`eld_tendermint_types::Error::MissingTrustedHeader`].
    pub fn add_gossiped_light(&self, evidence: &LightClientAttackEvidence) -> Result<(), Error> {
        evidence.validate_basic().map_err(Error::Invalid)?;
        Err(Error::Invalid(
            eld_tendermint_types::Error::MissingTrustedHeader,
        ))
    }

    /// Uncommitted evidence that fits in `max_bytes` of an `EvidenceList`.
    ///
    /// `max_bytes` below 0 means no cap. The reactor uses that to gossip every
    /// pending item.
    #[must_use]
    pub fn pending(&self, max_bytes: i64) -> Vec<Evidence> {
        let inner = lock(&self.inner);
        let Ok(rows) = inner.db.iter_prefix(&[PENDING]) else {
            return Vec::new();
        };
        let mut proto = eld_tendermint_proto::types::EvidenceList {
            evidence: Vec::new(),
        };
        let mut kept = Vec::new();
        for (_, value) in rows {
            let Ok(wrapped) = eld_tendermint_proto::types::Evidence::decode(value.as_slice())
            else {
                continue;
            };
            let Ok(evidence) = Evidence::try_from_evidence_proto(&wrapped) else {
                continue;
            };
            proto.evidence.push(evidence.to_evidence_proto());
            let size = i64::try_from(proto.encoded_len()).unwrap_or(i64::MAX);
            if max_bytes >= 0 && size > max_bytes {
                break;
            }
            kept.push(evidence);
        }
        kept
    }

    /// Move `evidence` from pending to committed so it is not proposed again.
    pub fn mark_committed(&self, evidence: &EvidenceList) {
        let inner = lock(&self.inner);
        for item in &evidence.evidence {
            let _ = inner.db.delete(&evidence_key(PENDING, item));
            let bytes = int64_value(item.height());
            let _ = inner.db.set_sync(&evidence_key(COMMITTED, item), &bytes);
        }
    }

    /// Replace the validator sets used by the next [`Self::add`].
    pub fn update_state(&self, state: &State) {
        lock(&self.inner).sets = sets_from(state);
    }
}

impl<D: Db> ProposalEvidence for Pool<D> {
    fn pending(&self, max_bytes: i64) -> Vec<Evidence> {
        Pool::pending(self, max_bytes)
    }

    fn mark_committed(&self, evidence: &EvidenceList) {
        Pool::mark_committed(self, evidence);
    }

    fn update_state(&self, state: &State) {
        Pool::update_state(self, state);
    }
}

/// Protobuf `google.protobuf.Int64Value`: field 1, varint.
fn int64_value(value: i64) -> Vec<u8> {
    let mut out = vec![0x08];
    let mut n = value as u64;
    loop {
        let mut byte = (n & 0x7f) as u8;
        n >>= 7;
        if n != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if n == 0 {
            break;
        }
    }
    out
}

fn sets_from(state: &State) -> Sets {
    Sets {
        chain_id: state.chain_id.as_str().to_owned(),
        validators: state.validators.copy(),
        last_validators: state.last_validators.copy(),
    }
}

fn verify_against_sets(sets: &Sets, evidence: &DuplicateVoteEvidence) -> Result<(), Error> {
    match evidence.verify(&sets.chain_id, &sets.validators) {
        Ok(()) => Ok(()),
        Err(eld_tendermint_types::Error::ValidatorNotInSet) => evidence
            .verify(&sets.chain_id, &sets.last_validators)
            .map_err(Error::Invalid),
        Err(err) => Err(Error::Invalid(err)),
    }
}

fn row_present(db: &impl Db, key: &[u8]) -> Result<bool, Error> {
    db.get(key)
        .map(|value| value.is_some())
        .map_err(|err| Error::Db(err.to_string()))
}

fn evidence_key(prefix: u8, evidence: &Evidence) -> Vec<u8> {
    let suffix = format!(
        "{:016X}/{}",
        evidence.height(),
        hex::encode_upper(evidence.hash().as_bytes())
    );
    let mut key = Vec::with_capacity(1 + suffix.len());
    key.push(prefix);
    key.extend(suffix.into_bytes());
    key
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
