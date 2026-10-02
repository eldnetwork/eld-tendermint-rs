//! `state.Store` load and save of the current state.
//!
//! The key is `stateKey` from `state/state.go`. Historical `validatorsKey` and
//! `consensusParamsKey` rows are not written. The state message holds the sets
//! this task reloads.

use prost::Message;

use eld_tendermint_store::Db;

use crate::error::Error;
use crate::state::State;

/// `stateKey`.
pub const STATE_KEY: &[u8] = b"stateKey";

/// Saves and loads [`State`] through the block-store [`Db`] trait.
pub struct StateStore<D: Db> {
    db: D,
}

impl<D: Db> StateStore<D> {
    #[must_use]
    pub fn new(db: D) -> Self {
        Self { db }
    }

    /// `Store.Load`. A missing or empty value is an empty store.
    ///
    /// # Panics
    ///
    /// Panics when `stateKey` is present and is not a `State`. The panic text
    /// includes `stateKey`. A database read failure panics the same way.
    #[must_use]
    pub fn load(&self) -> Option<State> {
        let bytes = match self.db.get(STATE_KEY) {
            Ok(bytes) => bytes?,
            Err(err) => panic!("state db get stateKey: {err}"),
        };
        let proto = eld_tendermint_proto::state::State::decode(bytes.as_slice())
            .unwrap_or_else(|err| panic!("corrupt state value at stateKey: {err}"));
        Some(
            State::try_from_proto(&proto)
                .unwrap_or_else(|err| panic!("corrupt state value at stateKey: {err}")),
        )
    }

    /// `Store.Save` of the state bytes. Uses `SetSync`.
    ///
    /// # Errors
    ///
    /// Returns a proto error, or the database error from `set_sync`.
    pub fn save(&self, state: &State) -> Result<(), Error> {
        let bytes = state.encode()?;
        self.db
            .set_sync(STATE_KEY, &bytes)
            .map_err(|err| Error::Db(err.to_string()))
    }
}
