//! Tendermint 0.34 state: `MakeGenesisState`, `InitChain`, and `ApplyBlock`.
//!
//! There is no mempool, evidence pool, event bus, or pruner. A missing `stateKey`
//! is an empty store. [`load_or_init_chain`] fills it once. A present value that
//! does not decode panics, and the message includes `stateKey`.

mod error;
mod execution;
mod init_chain;
mod state;
mod store;
mod txindex;

pub use error::Error;
pub use execution::{App, AppliedBlock, apply_block, validate_block};
pub use init_chain::load_or_init_chain;
pub use state::{State, StateVersion, TM_CORE_SEMVER, make_genesis_state};
pub use store::{STATE_KEY, StateStore};
pub use txindex::{CommitEvents, IndexTxs, TxIndex};
