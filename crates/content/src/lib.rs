//! Messages for content push, fetch, and capacity challenges.
//!
//! Channel [`CONTENT_CHANNEL`] is `0x50`. [`Reactor`] sends and receives those
//! messages. The node registers it and polls it with the other reactors.

mod reactor;

pub use reactor::{Inbound, Reactor};

use eld_tendermint_p2p::ChannelDescriptor;

/// Content and challenge channel.
pub const CONTENT_CHANNEL: u8 = 0x50;

/// Largest blob a push or response may carry. Pinboard messages stop at 1 MiB.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;

/// Reassembled message cap. One max blob, plus protobuf tags and a content id.
pub const RECV_MESSAGE_CAPACITY: usize = MAX_BLOB_BYTES + 4096;

/// Channel list to register on the switch before any peer is added.
///
/// Priority is 5, below consensus. The send queue holds 100 messages.
/// A message longer than [`RECV_MESSAGE_CAPACITY`] must not be queued.
#[must_use]
pub fn channel_descriptors() -> Vec<ChannelDescriptor> {
    vec![ChannelDescriptor {
        id: CONTENT_CHANNEL,
        priority: 5,
        send_queue_capacity: 100,
        recv_message_capacity: RECV_MESSAGE_CAPACITY,
    }]
}

/// `false` when a blob is over [`MAX_BLOB_BYTES`]. Check this before encoding a push.
#[must_use]
pub fn blob_fits(len: usize) -> bool {
    len <= MAX_BLOB_BYTES
}

/// `false` when `bytes` is longer than the channel will reassemble.
#[must_use]
pub fn message_fits(bytes: &[u8]) -> bool {
    bytes.len() <= RECV_MESSAGE_CAPACITY
}
