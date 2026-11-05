//! Content push, fetch, and capacity challenges on the p2p switch.
//!
//! Channel `0x50` carries `tendermint.content.Message`. [`Reactor::handle`]
//! decodes one message and calls [`App`](crate::App). A push, request, or
//! challenge that the app accepts is answered on the next [`Reactor::poll`].
//! The node starts its own push, fetch, or challenge through
//! [`ContentSendQueue`](crate::ContentSendQueue), which this reactor polls. Bytes that do not decode, an empty message, or a
//! blob over 1 MiB make `handle` return `false`. The node stops that peer.
//! An app refusal is logged and leaves the peer connected. A reply that does
//! not fit on the queue is logged and dropped.

use std::sync::Arc;

use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::content::{self, Message};
use eld_tendermint_types::{Level, log_line};
use prost::Message as ProstMessage;

use crate::app::App;
use crate::send_queue::{self, ContentSendQueue};
use crate::{Error, RECV_MESSAGE_CAPACITY, message_fits};

/// `ContentChannel`.
pub const CONTENT_CHANNEL: u8 = 0x50;

struct Shared<A: App> {
    app: A,
    send_queue: Arc<ContentSendQueue>,
}

/// Content channel on one node.
///
/// Cloning shares the app and the send queue. The node polls one clone and
/// the receive callback uses another. [`Self::send_queue`] is the same queue
/// the node holds.
pub struct Reactor<A: App> {
    shared: Arc<Shared<A>>,
}

impl<A: App> Clone for Reactor<A> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<A: App> Reactor<A> {
    /// `send_queue` is the queue the node writes. Poll sends from that same queue.
    #[must_use]
    pub fn from_shared(app: A, send_queue: Arc<ContentSendQueue>) -> Self {
        Self {
            shared: Arc::new(Shared { app, send_queue }),
        }
    }

    /// The queue [`Self::poll`] sends. Another clone of this `Arc` can enqueue
    /// while this reactor is polling.
    #[must_use]
    pub fn send_queue(&self) -> Arc<ContentSendQueue> {
        Arc::clone(&self.shared.send_queue)
    }

    /// Hold `message` for `peer_id` until [`Self::poll`].
    ///
    /// Returns `false` when the blob is over 1 MiB, the encoded message is over the
    /// channel cap, the message has no body, or the send queue is already full.
    /// Nothing is queued in those cases.
    #[must_use]
    pub fn queue(&self, peer_id: &str, message: &Message) -> bool {
        self.shared.send_queue.queue(peer_id, message)
    }

    /// Send queued messages to peers that are still connected.
    pub fn poll(&self, switch: &Switch) {
        self.shared.send_queue.poll(switch);
    }

    /// Decode one message and call [`App`](crate::App).
    ///
    /// Returns `false` when `bytes` is not a content message, the channel is not
    /// [`CONTENT_CHANNEL`], or a blob is over 1 MiB. The caller stops that peer.
    /// An [`Error`](crate::Error) from the app returns `true`. The peer stays
    /// connected, the refusal is logged, and no reply is queued. A reply that
    /// does not fit on the queue is logged and dropped. The peer stays connected.
    #[must_use]
    pub fn handle(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let Some(sum) = decode_inbound(ch_id, bytes) else {
            return false;
        };
        match reply_from(&self.shared.app, peer_id, sum) {
            Ok(Some(reply)) => {
                if !self.queue(peer_id, &reply) {
                    log_line(
                        Level::Error,
                        "content",
                        "reply dropped",
                        &[
                            ("peer", peer_id),
                            ("err", "queue full or message too large"),
                        ],
                    );
                }
            }
            Ok(None) => {}
            Err(err) => {
                let err = err.to_string();
                log_line(
                    Level::Error,
                    "content",
                    "app refused message",
                    &[("peer", peer_id), ("err", err.as_str())],
                );
            }
        }
        true
    }
}

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

fn decode_inbound(ch_id: u8, bytes: &[u8]) -> Option<content::message::Sum> {
    if ch_id != CONTENT_CHANNEL || !message_fits(bytes) {
        return None;
    }
    let message = Message::decode(bytes).ok()?;
    match message.sum {
        Some(sum) if send_queue::acceptable(&sum) => Some(sum),
        _ => None,
    }
}

fn reply_from<A: App>(
    app: &A,
    peer_id: &str,
    sum: content::message::Sum,
) -> Result<Option<Message>, Error> {
    match sum {
        content::message::Sum::ContentPush(push) => Ok(Some(wrap(
            content::message::Sum::ContentPushAck(app.handle_content_push(peer_id, push)?),
        ))),
        content::message::Sum::ContentRequest(request) => Ok(app
            .handle_content_request(peer_id, request)?
            .map(|response| wrap(content::message::Sum::ContentResponse(response)))),
        content::message::Sum::CapacityChallenge(challenge) => Ok(Some(wrap(
            content::message::Sum::CapacityChallengeResponse(
                app.handle_capacity_challenge(peer_id, challenge)?,
            ),
        ))),
        content::message::Sum::ContentPushAck(ack) => {
            app.handle_content_push_ack(peer_id, ack)?;
            Ok(None)
        }
        content::message::Sum::ContentResponse(response) => {
            app.handle_content_response(peer_id, response)?;
            Ok(None)
        }
        content::message::Sum::CapacityChallengeResponse(response) => {
            app.handle_capacity_challenge_response(peer_id, response)?;
            Ok(None)
        }
        content::message::Sum::Announce(announce) => {
            app.handle_announce(peer_id, announce)?;
            Ok(None)
        }
        content::message::Sum::CapacityProofSummaryQuery(_)
        | content::message::Sum::CapacityProofSummary(_) => Ok(None),
    }
}

fn wrap(sum: content::message::Sum) -> Message {
    Message { sum: Some(sum) }
}
