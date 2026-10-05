//! Tendermint 0.34 consensus for in-process validators.
//!
//! Round steps, proposals, and votes move by method call inside one process.
//! [`Reactor`] gossips `tendermint.consensus.Message` on the p2p switch.
//! The WAL is a CRC32C-framed head file that rotates to `wal.NNN` at 10 MiB.
//! A proposer with an evidence pool puts pending duplicate votes in the block.
//! A peer still inside the block store, and behind this node's consensus height,
//! catches up on the consensus reactor. Fast sync and state sync are not in this crate.

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
pub use round::{Msg, Node, NodeExtras, Step};
pub use wal::{Wal, end_height_message, proposal_message, vote_message};
