//! Tendermint 0.34 evidence pool.
//!
//! Duplicate-vote and light-client attack evidence are verified, stored, and gossiped
//! on channel `0x38`. A gossiped light-client attack is not stored: this pool has no
//! block store, so the caller of `Pool::add_light` supplies the trusted header.
//! There is no expiry pruning.

mod error;
mod pool;
mod reactor;

pub use error::Error;
pub use pool::{Pool, ProposalEvidence};
pub use reactor::{EVIDENCE_CHANNEL, Reactor, channel_descriptors};
