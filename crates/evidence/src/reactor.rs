//! Evidence gossip on channel `0x38`.
//!
//! Each broadcast is one `EvidenceList` holding a single duplicate vote.
//! A hash received from a peer is not sent back to that peer. A failed
//! `verify` leaves the peer connected. A protobuf that does not decode, or an
//! item that fails `ValidateBasic`, stops the peer.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::types::EvidenceList as ProtoList;
use eld_tendermint_store::Db;
use eld_tendermint_types::DuplicateVoteEvidence;
use prost::Message;

use crate::pool::Pool;

/// `EvidenceChannel`.
pub const EVIDENCE_CHANNEL: u8 = 0x38;

/// Go `maxMsgSize` for the evidence reactor.
const RECV_MESSAGE_CAPACITY: usize = 1_048_576;

/// Channel list from `Reactor.GetChannels`. Register this before `add_peer`.
///
/// The send queue is the Go default of 1. Priority is 6.
#[must_use]
pub fn channel_descriptors() -> Vec<ChannelDescriptor> {
    vec![ChannelDescriptor {
        id: EVIDENCE_CHANNEL,
        priority: 6,
        send_queue_capacity: 1,
        recv_message_capacity: RECV_MESSAGE_CAPACITY,
    }]
}

struct Inner<D: Db> {
    pool: Pool<D>,
    /// Hashes this peer already has: ones we sent, and ones they sent us.
    known: HashMap<String, HashSet<Vec<u8>>>,
    received: u64,
}

/// Local pool plus the hashes already exchanged with each peer.
pub struct Reactor<D: Db> {
    inner: Arc<Mutex<Inner<D>>>,
}

impl<D: Db> Clone for Reactor<D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<D: Db> Reactor<D> {
    #[must_use]
    pub fn new(pool: Pool<D>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                pool,
                known: HashMap::new(),
                received: 0,
            })),
        }
    }

    /// Decoded evidence messages this reactor has accepted from peers.
    #[must_use]
    pub fn messages_received(&self) -> u64 {
        lock(&self.inner).received
    }

    /// Send pending evidence each peer has not already sent us.
    pub fn poll(&self, switch: &Switch) {
        let mut inner = lock(&self.inner);
        let live: HashSet<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        inner.known.retain(|id, _| live.contains(id));
        let pending = inner.pool.pending(-1);
        let mut outbound = Vec::new();
        for peer_id in live {
            let known = inner.known.entry(peer_id.clone()).or_default();
            for evidence in &pending {
                let hash = evidence.hash().as_bytes().to_vec();
                if known.contains(&hash) {
                    continue;
                }
                known.insert(hash);
                outbound.push((peer_id.clone(), encode(evidence)));
            }
        }
        drop(inner);
        for (peer_id, bytes) in outbound {
            if !switch.send(&peer_id, EVIDENCE_CHANNEL, &bytes) {
                let hash = hash_of_message(&bytes);
                if let Some(hash) = hash {
                    lock(&self.inner)
                        .known
                        .entry(peer_id)
                        .or_default()
                        .remove(&hash);
                }
            }
        }
    }

    /// Decode one `EvidenceList` and add each duplicate vote.
    ///
    /// Returns `false` when `bytes` is not an `EvidenceList` or an item fails
    /// `ValidateBasic`. The caller stops that peer. A vote that fails `verify`
    /// is not stored, and this returns `true`.
    pub fn handle(&self, peer_id: &str, _ch_id: u8, bytes: &[u8]) -> bool {
        let Ok(list) = ProtoList::decode(bytes) else {
            return false;
        };
        let mut inner = lock(&self.inner);
        inner.received += 1;
        for item in list.evidence {
            let Ok(evidence) = DuplicateVoteEvidence::try_from_evidence_proto(&item) else {
                return false;
            };
            let hash = evidence.hash().as_bytes().to_vec();
            inner
                .known
                .entry(peer_id.to_owned())
                .or_default()
                .insert(hash);
            let _ = inner.pool.add(evidence);
        }
        true
    }
}

fn encode(evidence: &DuplicateVoteEvidence) -> Vec<u8> {
    ProtoList {
        evidence: vec![evidence.to_evidence_proto()],
    }
    .encode_to_vec()
}

fn hash_of_message(bytes: &[u8]) -> Option<Vec<u8>> {
    let list = ProtoList::decode(bytes).ok()?;
    let item = list.evidence.first()?;
    let evidence = DuplicateVoteEvidence::try_from_evidence_proto(item).ok()?;
    Some(evidence.hash().as_bytes().to_vec())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
