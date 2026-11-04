//! Messages for content push, fetch, and capacity challenges.
//!
//! Channel [`CONTENT_CHANNEL`] is `0x50`. [`Reactor`] calls [`App`] for each
//! message a peer sends, and polls [`ContentSendQueue`] for messages this node starts.
//! The node registers the reactor and polls it with the other reactors. Bytes
//! that are not one of these messages make [`Reactor::handle`] return `false`,
//! and the node stops that peer.

mod app;
mod error;
mod reactor;
mod send_queue;

pub use app::App;
pub use error::Error;
pub use reactor::{CONTENT_CHANNEL, Reactor, channel_descriptors};
pub use send_queue::ContentSendQueue;

/// Largest blob a push or response may carry. Pinboard messages stop at 1 MiB.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;

/// Reassembled message cap. One max blob, plus protobuf tags and a content id.
pub const RECV_MESSAGE_CAPACITY: usize = MAX_BLOB_BYTES + 4096;

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
