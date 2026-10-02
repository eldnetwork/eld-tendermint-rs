//! Tendermint 0.34 consensus for in-process validators.
//!
//! Round steps, proposals, and votes move by method call. There is no WAL, gossip
//! reactor, evidence pool, or fast sync. Timeouts use the consensus config durations
//! on a virtual clock.

mod error;
mod group;
mod round;
mod votes;

pub use error::Error;
pub use group::Group;
pub use round::{Msg, Node, Step};
