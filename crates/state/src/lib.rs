//! Tendermint 0.34 state: `MakeGenesisState` and `ApplyBlock`.
//!
//! There is no mempool, evidence pool, event bus, or pruner. A missing `stateKey`
//! is an empty store. A present value that does not decode panics, and the message
//! includes `stateKey`.

mod error;
mod execution;
mod state;
mod store;

pub use error::Error;
pub use execution::{App, apply_block};
pub use state::{State, StateVersion, TM_CORE_SEMVER, make_genesis_state};
pub use store::{STATE_KEY, StateStore};
