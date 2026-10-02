//! Tendermint 0.34 consensus for in-process validators.
//!
//! Round steps, proposals, and votes move by method call. The WAL is one
//! append-only file of CRC32C-framed `TimedWALMessage` records. There is no
//! gossip reactor, evidence pool, or fast sync. Timeouts use the consensus
//! config durations on a virtual clock.

mod error;
mod group;
mod round;
mod votes;
mod wal;

pub use error::Error;
pub use group::Group;
pub use round::{Msg, Node, Step};
pub use wal::{Wal, end_height_message, proposal_message, vote_message};
