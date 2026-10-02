//! Tendermint 0.34 consensus for in-process validators.
//!
//! Round steps, proposals, and votes move by method call inside one process.
//! [`Reactor`] gossips `tendermint.consensus.Message` on the p2p switch.
//! The WAL is one append-only file of CRC32C-framed `TimedWALMessage` records.
//! There is no evidence pool, catchup, or fast sync.

mod error;
mod group;
mod reactor;
mod round;
mod votes;
mod wal;

pub use error::Error;
pub use group::Group;
pub use reactor::{
    DATA_CHANNEL, Reactor, STATE_CHANNEL, VOTE_CHANNEL, VOTE_SET_BITS_CHANNEL, channel_descriptors,
};
pub use round::{Msg, Node, Step};
pub use wal::{Wal, end_height_message, proposal_message, vote_message};
