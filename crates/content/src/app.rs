//! Inbound content messages.
//!
//! [`Reactor`](crate::Reactor) calls [`App`] for each message that decodes.
//! The node implements this trait. The reactor does not call ABCI itself.

use eld_tendermint_proto::content::{
    Announce, CapacityChallenge, CapacityChallengeResponse, ContentPush, ContentPushAck,
    ContentRequest, ContentResponse,
};

use crate::Error;

/// Handler for one content-channel message.
///
/// `peer_id` is the id the p2p switch uses for the sender.
pub trait App: Send + Sync {
    /// A peer pushed a blob.
    ///
    /// The returned receipt is sent back to that peer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the blob is refused. No receipt is sent.
    fn handle_content_push(&self, peer_id: &str, msg: ContentPush)
    -> Result<ContentPushAck, Error>;

    /// A peer asked for a blob.
    ///
    /// `Ok(None)` means the blob is absent here. No response is sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the request is refused. No response is sent.
    fn handle_content_request(
        &self,
        peer_id: &str,
        msg: ContentRequest,
    ) -> Result<Option<ContentResponse>, Error>;

    /// A capacity validator challenged this node.
    ///
    /// The returned proof is sent back to that peer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when this node does not answer. No proof is sent.
    fn handle_capacity_challenge(
        &self,
        peer_id: &str,
        msg: CapacityChallenge,
    ) -> Result<CapacityChallengeResponse, Error>;

    /// A provider accepted a push this node sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the receipt is ignored.
    fn handle_content_push_ack(&self, peer_id: &str, msg: ContentPushAck) -> Result<(), Error>;

    /// A peer returned the blob this node asked for.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the blob is ignored.
    fn handle_content_response(&self, peer_id: &str, msg: ContentResponse) -> Result<(), Error>;

    /// A provider answered a challenge this node sent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the proof is ignored.
    fn handle_capacity_challenge_response(
        &self,
        peer_id: &str,
        msg: CapacityChallengeResponse,
    ) -> Result<(), Error>;

    /// A peer announced that it has this content.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the announcement is ignored.
    fn handle_announce(&self, peer_id: &str, msg: Announce) -> Result<(), Error>;

    /// The summary the local capacity validator signed for `epoch`.
    ///
    /// `Ok(None)` means this epoch has no signed summary yet. The bytes are the
    /// JSON encoding of the app's `CapacityProofSummary`. This is a local read.
    /// It is not a peer message.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Rejected`] when the app does not answer.
    fn capacity_proof_summary(&self, epoch: i64) -> Result<Option<Vec<u8>>, Error>;
}
