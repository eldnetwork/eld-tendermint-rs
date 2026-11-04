//! Messages waiting to be sent on the content channel.
//!
//! The node keeps one [`ContentSendQueue`] and the reactor polls that same queue.
//! [`ContentSendQueue::push`], [`ContentSendQueue::request`], and
//! [`ContentSendQueue::challenge`] start a message to one peer.
//! [`Reactor::poll`](crate::Reactor::poll) writes it.

use std::collections::{HashSet, VecDeque};
use std::sync::{Mutex, MutexGuard};

use eld_tendermint_p2p::Switch;
use eld_tendermint_proto::content::{
    self, CapacityChallenge, ContentPush, ContentRequest, Message,
};
use eld_tendermint_types::{Level, log_line};
use prost::Message as ProstMessage;

use crate::{CONTENT_CHANNEL, blob_fits, message_fits};

/// Waiting messages. Matches the channel send queue of 100.
const QUEUE_CAP: usize = 100;

struct Queued {
    peer_id: String,
    bytes: Vec<u8>,
}

/// Content messages this node will send on channel `0x50`.
///
/// Share one queue with [`Arc`](std::sync::Arc). The node keeps it. The content
/// reactor polls it.
pub struct ContentSendQueue {
    inner: Mutex<VecDeque<Queued>>,
}

impl ContentSendQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
        }
    }

    /// Hold `message` for `peer_id` until the reactor polls.
    ///
    /// Returns `false` when the blob is over 1 MiB, the encoded message is over the
    /// channel cap, the message has no body, or 100 messages are already waiting.
    /// Nothing is queued in those cases. A full queue is logged.
    #[must_use]
    pub(crate) fn queue(&self, peer_id: &str, message: &Message) -> bool {
        let Some(bytes) = encode(message) else {
            return false;
        };
        self.enqueue(peer_id, bytes)
    }

    /// Start a push to `peer_id`.
    ///
    /// Returns `false` when the blob is over 1 MiB, the encoded message is over the
    /// channel cap, or 100 messages are already waiting. Nothing is queued then.
    /// A full queue is logged.
    #[must_use]
    pub fn push(&self, peer_id: &str, msg: &ContentPush) -> bool {
        self.queue(
            peer_id,
            &message(content::message::Sum::ContentPush(msg.clone())),
        )
    }

    /// Start a fetch from `peer_id`.
    ///
    /// Returns `false` when the encoded message is over the channel cap or 100
    /// messages are already waiting. Nothing is queued then. A full queue is logged.
    #[must_use]
    pub fn request(&self, peer_id: &str, msg: &ContentRequest) -> bool {
        self.queue(
            peer_id,
            &message(content::message::Sum::ContentRequest(msg.clone())),
        )
    }

    /// Start a capacity challenge to `peer_id`.
    ///
    /// Returns `false` when the encoded message is over the channel cap or 100
    /// messages are already waiting. Nothing is queued then. A full queue is logged.
    #[must_use]
    pub fn challenge(&self, peer_id: &str, msg: &CapacityChallenge) -> bool {
        self.queue(
            peer_id,
            &message(content::message::Sum::CapacityChallenge(msg.clone())),
        )
    }

    /// Send queued messages to peers that are still connected.
    ///
    /// A full channel queue leaves the message here for the next poll. A peer that
    /// has gone is dropped from the queue.
    pub(crate) fn poll(&self, switch: &Switch) {
        let live: HashSet<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        let pending = {
            let mut inner = lock(&self.inner);
            std::mem::take(&mut *inner)
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
        again.append(&mut inner);
        *inner = again;
    }

    fn enqueue(&self, peer_id: &str, bytes: Vec<u8>) -> bool {
        let mut inner = lock(&self.inner);
        if inner.len() >= QUEUE_CAP {
            drop(inner);
            log_line(
                Level::Error,
                "content",
                "send queue full",
                &[("peer", peer_id)],
            );
            return false;
        }
        inner.push_back(Queued {
            peer_id: peer_id.to_owned(),
            bytes,
        });
        true
    }
}

impl Default for ContentSendQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// `false` when a push or response blob is over 1 MiB.
pub(crate) fn acceptable(sum: &content::message::Sum) -> bool {
    match sum {
        content::message::Sum::ContentPush(push) => blob_fits(push.blob.len()),
        content::message::Sum::ContentResponse(response) => blob_fits(response.blob.len()),
        _ => true,
    }
}

fn message(sum: content::message::Sum) -> Message {
    Message { sum: Some(sum) }
}

fn encode(message: &Message) -> Option<Vec<u8>> {
    let sum = message.sum.as_ref()?;
    if !acceptable(sum) {
        return None;
    }
    let bytes = message.encode_to_vec();
    message_fits(&bytes).then_some(bytes)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
