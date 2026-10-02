//! In-flight heights for one fast-sync peer.
//!
//! `maxPendingRequestsPerPeer` is 20. A height stays assigned until the block
//! arrives, the peer says it has no block, or the peer is gone.

use std::collections::{HashMap, HashSet};

use eld_tendermint_types::Block;

/// `maxPendingRequestsPerPeer`.
pub(crate) const MAX_PENDING_PER_PEER: usize = 20;

pub(crate) struct Peer {
    pub base: i64,
    pub height: i64,
}

pub(crate) struct Buffered {
    pub peer_id: String,
    pub block: Block,
}

/// What to do with a block that answers an in-flight request.
pub(crate) enum Incoming {
    /// `store.height() + 1`. Apply it now.
    Next(Box<Block>),
    /// Later height inside the requested window.
    Later,
    /// Already applied. The request is cleared.
    Duplicate,
}

pub(crate) struct Pool {
    peers: HashMap<String, Peer>,
    greeted: HashSet<String>,
    /// Height assigned to one peer.
    inflight: HashMap<i64, String>,
    /// Peers that answered `NoBlockResponse` for a height.
    refused: HashMap<i64, HashSet<String>>,
    buffered: HashMap<i64, Buffered>,
    requests_sent: u64,
}

impl Pool {
    pub(crate) fn new() -> Self {
        Self {
            peers: HashMap::new(),
            greeted: HashSet::new(),
            inflight: HashMap::new(),
            refused: HashMap::new(),
            buffered: HashMap::new(),
            requests_sent: 0,
        }
    }

    pub(crate) fn requests_sent(&self) -> u64 {
        self.requests_sent
    }

    pub(crate) fn peer(&self, peer_id: &str) -> Option<&Peer> {
        self.peers.get(peer_id)
    }

    /// First time this peer is seen. The caller sends `StatusRequest`.
    pub(crate) fn greet(&mut self, peer_id: &str) -> bool {
        self.greeted.insert(peer_id.to_owned())
    }

    pub(crate) fn set_range(&mut self, peer_id: &str, base: i64, height: i64) {
        self.peers.insert(peer_id.to_owned(), Peer { base, height });
    }

    /// Drop peers that are no longer running and free their in-flight heights.
    pub(crate) fn forget_absent(&mut self, live: &HashSet<String>) {
        self.greeted.retain(|id| live.contains(id));
        self.peers.retain(|id, _| live.contains(id));
        self.inflight.retain(|_, peer| live.contains(peer));
    }

    /// Heights this store still needs, up to 20 in flight for each taller peer.
    pub(crate) fn requests(&self, our_height: i64) -> Vec<(String, i64)> {
        let mut out = Vec::new();
        for (peer_id, peer) in &self.peers {
            if peer.height <= our_height {
                continue;
            }
            let mut pending = self.pending(peer_id);
            let mut height = (our_height + 1).max(peer.base);
            while pending < MAX_PENDING_PER_PEER && height <= peer.height {
                if self.claimed(height) || self.refused(peer_id, height) {
                    height += 1;
                    continue;
                }
                out.push((peer_id.clone(), height));
                pending += 1;
                height += 1;
            }
        }
        out
    }

    pub(crate) fn mark_requested(&mut self, peer_id: &str, height: i64) {
        self.inflight.insert(height, peer_id.to_owned());
        self.requests_sent += 1;
    }

    /// The send did not queue. The height can be assigned again.
    pub(crate) fn cancel_request(&mut self, peer_id: &str, height: i64) {
        if self.inflight.get(&height).is_some_and(|id| id == peer_id) {
            self.inflight.remove(&height);
            self.requests_sent = self.requests_sent.saturating_sub(1);
        }
    }

    /// `None` when `height` was not requested. That is a gap.
    pub(crate) fn take_block(
        &mut self,
        peer_id: &str,
        our_height: i64,
        block: Block,
    ) -> Option<Incoming> {
        let height = block.header.height;
        if !self.inflight.contains_key(&height) {
            return None;
        }
        self.inflight.remove(&height);
        if height == our_height + 1 {
            return Some(Incoming::Next(Box::new(block)));
        }
        if height > our_height + 1 {
            self.buffered.insert(
                height,
                Buffered {
                    peer_id: peer_id.to_owned(),
                    block,
                },
            );
            return Some(Incoming::Later);
        }
        Some(Incoming::Duplicate)
    }

    pub(crate) fn take_no_block(&mut self, peer_id: &str, height: i64) {
        if self.inflight.get(&height).is_some_and(|id| id == peer_id) {
            self.inflight.remove(&height);
        }
        self.refused
            .entry(height)
            .or_default()
            .insert(peer_id.to_owned());
    }

    pub(crate) fn pop_next(&mut self, height: i64) -> Option<Buffered> {
        self.buffered.remove(&height)
    }

    fn pending(&self, peer_id: &str) -> usize {
        self.inflight.values().filter(|id| *id == peer_id).count()
    }

    fn claimed(&self, height: i64) -> bool {
        self.inflight.contains_key(&height) || self.buffered.contains_key(&height)
    }

    fn refused(&self, peer_id: &str, height: i64) -> bool {
        self.refused
            .get(&height)
            .is_some_and(|peers| peers.contains(peer_id))
    }
}
