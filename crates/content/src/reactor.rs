//! Send and receive content messages on channel [`CONTENT_CHANNEL`](crate::CONTENT_CHANNEL).
//!
//! [`Reactor::queue`] holds a message until [`Reactor::poll`] puts it on the switch.
//! [`Reactor::handle`] keeps a message a peer sent. Bytes that are not one of these
//! messages make `handle` return `false`, and the node stops that peer.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_p2p::Switch;
use eld_tendermint_proto::content::{self, Message};
use prost::Message as ProstMessage;

use crate::{CONTENT_CHANNEL, blob_fits, message_fits};

/// Outbound and inbound queues. Matches the channel send queue of 100.
const QUEUE_CAP: usize = 100;

struct Queued {
    peer_id: String,
    bytes: Vec<u8>,
}

/// One message a peer sent on the content channel.
pub struct Inbound {
    pub peer_id: String,
    pub message: Message,
}

struct Inner {
    outbound: VecDeque<Queued>,
    inbound: VecDeque<Inbound>,
}

/// Content channel on one node.
///
/// Cloning shares the queues. The node polls one clone and the receive callback uses another.
pub struct Reactor {
    inner: Arc<Mutex<Inner>>,
}

impl Clone for Reactor {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Reactor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                outbound: VecDeque::new(),
                inbound: VecDeque::new(),
            })),
        }
    }

    /// Hold `message` for `peer_id` until [`Self::poll`].
    ///
    /// Returns `false` when the blob is over 1 MiB, the encoded message is over the
    /// channel cap, the message has no body, or the outbound queue is already full.
    /// Nothing is queued in those cases.
    #[must_use]
    pub fn queue(&self, peer_id: &str, message: &Message) -> bool {
        if !acceptable(message) {
            return false;
        }
        let bytes = message.encode_to_vec();
        if !message_fits(&bytes) {
            return false;
        }
        let mut inner = lock(&self.inner);
        if inner.outbound.len() >= QUEUE_CAP {
            return false;
        }
        inner.outbound.push_back(Queued {
            peer_id: peer_id.to_owned(),
            bytes,
        });
        true
    }

    /// Send queued messages to peers that are still connected.
    ///
    /// A full channel queue leaves the message here for the next poll. A peer that
    /// has gone is dropped from the queue.
    pub fn poll(&self, switch: &Switch) {
        let live: HashSet<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        let pending = {
            let mut inner = lock(&self.inner);
            std::mem::take(&mut inner.outbound)
        };
        let mut again = VecDeque::new();
        for queued in pending {
            if !live.contains(&queued.peer_id) {
                continue;
            }
            if !switch.try_send(&queued.peer_id, CONTENT_CHANNEL, &queued.bytes) {
                again.push_back(queued);
            }
        }
        if again.is_empty() {
            return;
        }
        let mut inner = lock(&self.inner);
        again.append(&mut inner.outbound);
        inner.outbound = again;
    }

    /// Decode one message and keep it for [`Self::drain_inbound`].
    ///
    /// Returns `false` when `bytes` is not a content message, the channel is not
    /// [`CONTENT_CHANNEL`](crate::CONTENT_CHANNEL), or a blob is over 1 MiB. The
    /// caller stops that peer. When the inbound queue is full, the oldest message
    /// is dropped so this queue cannot grow without a limit.
    #[must_use]
    pub fn handle(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        if ch_id != CONTENT_CHANNEL || !message_fits(bytes) {
            return false;
        }
        let Ok(message) = Message::decode(bytes) else {
            return false;
        };
        if !acceptable(&message) {
            return false;
        }
        let mut inner = lock(&self.inner);
        if inner.inbound.len() >= QUEUE_CAP {
            inner.inbound.pop_front();
        }
        inner.inbound.push_back(Inbound {
            peer_id: peer_id.to_owned(),
            message,
        });
        true
    }

    /// Messages received since the last drain, oldest first.
    #[must_use]
    pub fn drain_inbound(&self) -> Vec<Inbound> {
        let mut inner = lock(&self.inner);
        inner.inbound.drain(..).collect()
    }
}

impl Default for Reactor {
    fn default() -> Self {
        Self::new()
    }
}

fn acceptable(message: &Message) -> bool {
    match &message.sum {
        Some(content::message::Sum::ContentPush(push)) => blob_fits(push.blob.len()),
        Some(content::message::Sum::ContentResponse(response)) => blob_fits(response.blob.len()),
        Some(_) => true,
        None => false,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
