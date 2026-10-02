//! Tendermint 0.34 evidence pool.
//!
//! Duplicate-vote evidence is verified, stored, and gossiped on channel `0x38`.
//! Light-client attack evidence is not accepted. There is no expiry pruning.

mod error;
mod pool;
mod reactor;

pub use error::Error;
pub use pool::{Pool, ProposalEvidence};
pub use reactor::{EVIDENCE_CHANNEL, Reactor, channel_descriptors};
