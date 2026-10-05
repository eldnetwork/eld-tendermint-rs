//! Consensus gossip on the p2p switch.
//!
//! Four channels carry `tendermint.consensus.Message`. Proposals, block parts, and
//! votes are delivered into the local [`Node`]s. A peer one or two consensus
//! heights behind is sent that block's parts on `0x21` and its seen-commit
//! precommits on `0x22`. `NewValidBlock` updates the peer's part-set header.
//! There is no `VoteSetMaj23` reply.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eld_tendermint_mempool::App as MempoolApp;
use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::consensus::{
    BlockPart, HasVote, Message, NewRoundStep, NewValidBlock, Proposal as ProtoProposal,
    Vote as ProtoVote, VoteSetBits, message,
};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::App as ExecApp;
use eld_tendermint_store::{Db, MemDb};
use eld_tendermint_types::{BitArray, Part, PartSetHeader, Proposal, Vote};
use prost::Message as ProstMessage;

use crate::round::{Msg, Node, OutboundValidBlock, Scheduled, Step};

/// `StateChannel`.
pub const STATE_CHANNEL: u8 = 0x20;
/// `DataChannel`.
pub const DATA_CHANNEL: u8 = 0x21;
/// `VoteChannel`.
pub const VOTE_CHANNEL: u8 = 0x22;
/// `VoteSetBitsChannel`.
pub const VOTE_SET_BITS_CHANNEL: u8 = 0x23;

const MAX_MSG_SIZE: usize = 1024 * 1024;

/// Channel list from `Reactor.GetChannels`. Register this before `add_peer`.
#[must_use]
pub fn channel_descriptors() -> Vec<ChannelDescriptor> {
    vec![
        descriptor(STATE_CHANNEL, 6, 100),
        descriptor(DATA_CHANNEL, 10, 100),
        descriptor(VOTE_CHANNEL, 7, 100),
        descriptor(VOTE_SET_BITS_CHANNEL, 1, 2),
    ]
}

fn descriptor(id: u8, priority: u32, send_queue_capacity: usize) -> ChannelDescriptor {
    ChannelDescriptor {
        id,
        priority,
        send_queue_capacity,
        recv_message_capacity: MAX_MSG_SIZE,
    }
}

struct PeerState {
    height: i64,
    round: i32,
    step: u32,
    proposal_hash: Option<Vec<u8>>,
    has_proposal: bool,
    proposal_part_set_header: Option<PartSetHeader>,
    parts: Option<BitArray>,
    prevotes: Option<BitArray>,
    precommits: Option<BitArray>,
    catchup_votes: Option<BitArray>,
}

impl PeerState {
    fn new() -> Self {
        Self {
            height: 0,
            round: 0,
            step: 0,
            proposal_hash: None,
            has_proposal: false,
            proposal_part_set_header: None,
            parts: None,
            prevotes: None,
            precommits: None,
            catchup_votes: None,
        }
    }

    fn apply_round_step(&mut self, msg: &NewRoundStep) {
        if (msg.height, msg.round, msg.step) <= (self.height, self.round, self.step) {
            return;
        }
        let reset = msg.height != self.height || msg.round != self.round;
        let height_changed = msg.height != self.height;
        self.height = msg.height;
        self.round = msg.round;
        self.step = msg.step;
        if height_changed {
            self.catchup_votes = None;
        }
        if reset {
            self.proposal_hash = None;
            self.has_proposal = false;
            self.proposal_part_set_header = None;
            self.parts = None;
            self.prevotes = None;
            self.precommits = None;
        }
    }

    /// `SetHasProposal`. A part set already stored for this height and round stays.
    fn note_proposal(&mut self, proposal: &Proposal) {
        if self.height != proposal.height || self.round != proposal.round || self.has_proposal {
            return;
        }
        self.has_proposal = true;
        self.proposal_hash = Some(proposal.block_id.hash.clone());
        if self.parts.is_some() {
            return;
        }
        self.proposal_part_set_header = Some(proposal.block_id.part_set_header.clone());
        ensure_parts(self, proposal.block_id.part_set_header.total);
    }
}

#[derive(Clone)]
struct TimeoutWatch {
    key: (i64, i32, u32, i64),
    started: Instant,
}

struct Inner<E: ExecApp, C: MempoolApp, D: Db> {
    nodes: Vec<Node<E, C, D>>,
    peers: HashMap<String, PeerState>,
    announced: Vec<Option<(i64, i32, u32)>>,
    proposals: Vec<Proposal>,
    votes: Vec<Vote>,
    timeouts: Vec<Option<TimeoutWatch>>,
    validator_count: i64,
}

/// Local validators plus the peer state used to gossip on a [`Switch`].
pub struct Reactor<E: ExecApp, C: MempoolApp, D: Db = MemDb> {
    inner: Arc<Mutex<Inner<E, C, D>>>,
}

impl<E, C, D> Clone for Reactor<E, C, D>
where
    E: ExecApp,
    C: MempoolApp,
    D: Db,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<E, C, D> Reactor<E, C, D>
where
    E: ExecApp + Send,
    C: MempoolApp + Send,
    D: Db,
{
    #[must_use]
    pub fn new(nodes: Vec<Node<E, C, D>>) -> Self {
        let validator_count = i64::try_from(nodes.first().map(Node::validator_count).unwrap_or(0))
            .unwrap_or(i64::MAX);
        let n = nodes.len();
        Self {
            inner: Arc::new(Mutex::new(Inner {
                nodes,
                peers: HashMap::new(),
                announced: vec![None; n],
                proposals: Vec::new(),
                votes: Vec::new(),
                timeouts: vec![None; n],
                validator_count,
            })),
        }
    }

    /// Drain local outboxes, gossip anything peers are missing, and fire due timeouts.
    pub fn poll(&self, switch: &Switch) {
        let mut inner = lock(&self.inner);
        inner.fire_due_timeouts();
        inner.drain_outboxes();
        inner.sync_peers(switch);
        inner.announce(switch);
        inner.gossip(switch);
    }

    /// Send rounds, proposals, and catchup without firing timeouts.
    ///
    /// [`Self::poll`] is what starts the next round. This only pushes messages.
    pub fn gossip(&self, switch: &Switch) {
        let mut inner = lock(&self.inner);
        inner.sync_peers(switch);
        inner.announce(switch);
        inner.gossip(switch);
    }

    /// Decode one reactor message and feed the local validators.
    ///
    /// Returns `false` when `bytes` is not a `consensus.Message`. The caller stops
    /// that peer. A bad signature or a bad part proof returns `true`.
    pub fn handle(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let mut inner = lock(&self.inner);
        inner.handle(peer_id, ch_id, bytes)
    }

    /// Header hash of the block saved at `height` on the first local node that has it.
    #[must_use]
    pub fn committed_hash(&self, height: i64) -> Option<Vec<u8>> {
        lock(&self.inner)
            .nodes
            .iter()
            .find_map(|node| node.committed_hash(height))
    }

    /// Latest signature stored by a local validator, if one has signed.
    #[must_use]
    pub fn last_signature(&self) -> Option<Vec<u8>> {
        lock(&self.inner)
            .nodes
            .iter()
            .find_map(Node::last_signature)
    }

    /// Every local block store is at least `height`.
    #[must_use]
    pub fn committed(&self, height: i64) -> bool {
        let inner = lock(&self.inner);
        !inner.nodes.is_empty() && inner.nodes.iter().all(|node| node.store_height() >= height)
    }

    /// Sum of prevotes at `round` across the local validators.
    #[must_use]
    pub fn prevote_count(&self, round: i32) -> usize {
        lock(&self.inner)
            .nodes
            .iter()
            .map(|node| node.prevote_count(round))
            .sum()
    }

    /// Round step of the first local validator.
    #[must_use]
    pub fn step(&self) -> Step {
        lock(&self.inner)
            .nodes
            .first()
            .map(Node::step)
            .unwrap_or(Step::NewHeight)
    }

    /// Consensus height of the first local validator, or 0 when there is none.
    #[must_use]
    pub fn height(&self) -> i64 {
        lock(&self.inner)
            .nodes
            .first()
            .map(Node::height)
            .unwrap_or(0)
    }

    /// Round of the first local validator, or 0 when there is none.
    #[must_use]
    pub fn round(&self) -> i32 {
        lock(&self.inner)
            .nodes
            .first()
            .map(Node::round)
            .unwrap_or(0)
    }

    /// Block hash of the first local validator's current proposal.
    #[must_use]
    pub fn proposal_hash(&self) -> Option<Vec<u8>> {
        lock(&self.inner)
            .nodes
            .first()
            .and_then(Node::proposal_hash)
    }
}

impl<E: ExecApp, C: MempoolApp, D: Db> Inner<E, C, D> {
    fn fire_due_timeouts(&mut self) {
        let now = Instant::now();
        for index in 0..self.nodes.len() {
            let Some(timeout) = self.nodes[index].timeout.clone() else {
                self.timeouts[index] = None;
                continue;
            };
            let key = timeout_key(&timeout);
            match &self.timeouts[index] {
                Some(watch) if watch.key == key => {
                    let delay =
                        Duration::from_nanos(u64::try_from(timeout.delay_nanos).unwrap_or(0));
                    if watch.started.elapsed() >= delay {
                        self.timeouts[index] = None;
                        self.nodes[index].timeout = None;
                        self.nodes[index].on_timeout(&timeout);
                    }
                }
                _ => {
                    self.timeouts[index] = Some(TimeoutWatch { key, started: now });
                }
            }
        }
    }

    fn drain_outboxes(&mut self) {
        for _ in 0..10_000 {
            let mut batch = Vec::new();
            for node in &mut self.nodes {
                batch.append(&mut node.take_outbox());
            }
            if batch.is_empty() {
                return;
            }
            for msg in batch {
                for node in &mut self.nodes {
                    node.deliver(msg.clone());
                }
                self.remember(msg);
            }
        }
    }

    fn remember(&mut self, msg: Msg) {
        match msg {
            Msg::Proposal(proposal) => self.remember_proposal(proposal),
            // Parts are read from `Node::proposal_parts` on the next poll.
            Msg::Part(_) => {}
            Msg::Vote(vote) => self.remember_vote(vote),
        }
    }

    fn remember_proposal(&mut self, proposal: Proposal) {
        if !self
            .proposals
            .iter()
            .any(|have| have.signature == proposal.signature)
        {
            self.proposals.push(proposal);
        }
    }

    fn remember_vote(&mut self, vote: Vote) {
        let seen = self.votes.iter().any(|have| {
            have.height == vote.height
                && have.round == vote.round
                && have.vote_type == vote.vote_type
                && have.validator_index == vote.validator_index
        });
        if !seen {
            self.votes.push(vote);
        }
    }

    fn sync_peers(&mut self, switch: &Switch) {
        for info in switch.peers() {
            if self.peers.contains_key(&info.id) {
                continue;
            }
            self.peers.insert(info.id.clone(), PeerState::new());
            for node in &self.nodes {
                let _ = switch.try_send(&info.id, STATE_CHANNEL, &round_step_message(node));
            }
        }
    }

    fn announce(&mut self, switch: &Switch) {
        for (index, node) in self.nodes.iter().enumerate() {
            let state = (node.height(), node.round(), node.step().as_wal());
            if self.announced[index] == Some(state) {
                continue;
            }
            self.announced[index] = Some(state);
            for info in switch.peers() {
                let _ = switch.try_send(&info.id, STATE_CHANNEL, &round_step_message(node));
            }
        }
    }

    fn gossip(&mut self, switch: &Switch) {
        self.broadcast_valid_blocks(switch);
        let peer_ids: Vec<String> = self.peers.keys().cloned().collect();
        for peer_id in peer_ids {
            // Catchup goes out before the live round so a full data queue cannot
            // drop the block the peer is missing.
            self.send_catchup(switch, &peer_id);
            self.send_proposals(switch, &peer_id);
            self.send_parts(switch, &peer_id);
            self.send_votes(switch, &peer_id);
        }
    }

    /// Parts and seen-commit votes for the block a peer is still deciding.
    ///
    /// Only a gap of one or two consensus heights is filled here. A wider gap is
    /// left to fast sync. Nothing is sent for a height the peer has already passed.
    fn send_catchup(&mut self, switch: &Switch, peer_id: &str) {
        let (peer_height, peer_round, our_height) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            let our_height = self.nodes.iter().map(Node::height).max().unwrap_or(0);
            (peer.height, peer.round, our_height)
        };
        if peer_height <= 0 {
            return;
        }
        let gap = our_height.saturating_sub(peer_height);
        if gap != 1 && gap != 2 {
            return;
        }
        let Some(source) = self.nodes.iter().position(|node| {
            node.store_height() >= peer_height && node.block_part_count(peer_height).is_some()
        }) else {
            return;
        };
        let Some(total) = self.nodes[source].block_part_count(peer_height) else {
            return;
        };
        if total == 0 {
            return;
        }
        let votes = self.nodes[source].seen_commit_votes(peer_height);
        if votes.is_empty() {
            return;
        }
        let vote_bits = votes
            .iter()
            .map(|vote| i64::from(vote.validator_index) + 1)
            .max()
            .unwrap_or(1);
        let Some(block_header) = self.nodes[source].block_part_set_header(peer_height) else {
            return;
        };
        {
            let Some(peer) = self.peers.get_mut(peer_id) else {
                return;
            };
            if peer
                .catchup_votes
                .as_ref()
                .is_none_or(|bits| bits.size() < vote_bits)
            {
                peer.catchup_votes = BitArray::new(vote_bits);
            }
        }
        let pending_votes: Vec<Vote> = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            votes
                .into_iter()
                .filter(|vote| {
                    peer.catchup_votes
                        .as_ref()
                        .is_none_or(|bits| !bits.get_index(i64::from(vote.validator_index)))
                })
                .collect()
        };
        for vote in pending_votes {
            let index = vote.validator_index;
            if send_vote(switch, peer_id, &vote) {
                self.mark_catchup_vote(peer_id, index);
            }
        }
        // `ProposalBlockParts == nil`: `InitProposalBlockParts`, then the next pass
        // sends one part. A different header is `gossipDataForCatchup`'s mismatch sleep.
        let uninitialized = self
            .peers
            .get(peer_id)
            .is_some_and(|peer| peer.parts.is_none());
        if uninitialized {
            if let Some(peer) = self.peers.get_mut(peer_id) {
                peer.proposal_part_set_header = Some(block_header);
                ensure_parts(peer, total);
            }
            return;
        }
        let matches = self
            .peers
            .get(peer_id)
            .is_some_and(|peer| peer.proposal_part_set_header.as_ref() == Some(&block_header));
        if !matches {
            return;
        }
        let sent = self.peers.get(peer_id).and_then(|peer| peer.parts.clone());
        for index in 0..total {
            if sent
                .as_ref()
                .is_some_and(|bits| bits.get_index(i64::from(index)))
            {
                continue;
            }
            let Some(part) = self.nodes[source].block_part(peer_height, index) else {
                continue;
            };
            if send_block_part(switch, peer_id, peer_height, peer_round, &part) {
                self.set_has_proposal_block_part(peer_id, peer_height, peer_round, index);
            }
            return;
        }
    }

    /// `SetHasProposalBlockPart`. No-ops when the peer has moved to another height or round.
    fn set_has_proposal_block_part(&mut self, peer_id: &str, height: i64, round: i32, index: u32) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if peer.height != height || peer.round != round {
            return;
        }
        if let Some(bits) = peer.parts.as_mut() {
            bits.set_index(i64::from(index), true);
        }
    }

    fn mark_catchup_vote(&mut self, peer_id: &str, index: i32) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if let Some(bits) = peer.catchup_votes.as_mut() {
            bits.set_index(i64::from(index), true);
        }
    }

    fn send_proposals(&mut self, switch: &Switch, peer_id: &str) {
        let Some(peer) = self.peers.get(peer_id) else {
            return;
        };
        let pending: Vec<Proposal> = self
            .proposals
            .iter()
            .filter(|proposal| {
                peer.height == proposal.height && peer.round == proposal.round && !peer.has_proposal
            })
            .cloned()
            .collect();
        for proposal in pending {
            let bytes = Message {
                sum: Some(message::Sum::Proposal(ProtoProposal {
                    proposal: Some(proposal.to_proto()),
                })),
            }
            .encode_to_vec();
            if switch.try_send(peer_id, DATA_CHANNEL, &bytes) {
                if let Some(peer) = self.peers.get_mut(peer_id) {
                    peer.note_proposal(&proposal);
                }
            }
        }
    }

    /// One proposal part whose part-set header matches the peer's.
    ///
    /// Go matches parts on the header hash, so the peer's round can differ.
    /// The message still carries this node's height and round.
    fn send_parts(&mut self, switch: &Switch, peer_id: &str) {
        let (height, header, peer_bits) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            let Some(header) = peer.proposal_part_set_header.clone() else {
                return;
            };
            (peer.height, header, peer.parts.clone())
        };
        let Some((round, part)) = self.nodes.iter().find_map(|node| {
            if node.height() != height {
                return None;
            }
            if node.proposal_part_set_header().as_ref() != Some(&header) {
                return None;
            }
            if !node.proposal_parts_has_header(&header) {
                return None;
            }
            let bits = node.proposal_parts_bits()?;
            let index = (0..bits.size()).find(|&index| {
                bits.get_index(index)
                    && peer_bits.as_ref().is_none_or(|have| !have.get_index(index))
            })?;
            let index = u32::try_from(index).ok()?;
            let part = node.proposal_part(index)?;
            Some((node.round(), part))
        }) else {
            return;
        };
        if send_block_part(switch, peer_id, height, round, &part) {
            self.mark_part(peer_id, part.index);
        }
    }

    fn send_votes(&mut self, switch: &Switch, peer_id: &str) {
        let Some(peer) = self.peers.get(peer_id) else {
            return;
        };
        let height = peer.height;
        let round = peer.round;
        let pending: Vec<Vote> = self
            .votes
            .iter()
            .filter(|vote| vote.height == height && vote.round == round)
            .filter(|vote| !vote_bit(peer, vote))
            .cloned()
            .collect();
        for vote in pending {
            let bytes = Message {
                sum: Some(message::Sum::Vote(ProtoVote {
                    vote: Some(vote.to_proto()),
                })),
            }
            .encode_to_vec();
            if switch.try_send(peer_id, VOTE_CHANNEL, &bytes) {
                self.mark_vote(peer_id, &vote);
            }
        }
    }

    fn mark_part(&mut self, peer_id: &str, index: u32) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        let total = peer
            .proposal_part_set_header
            .as_ref()
            .map(|header| header.total)
            .unwrap_or(index.saturating_add(1))
            .max(1);
        ensure_parts(peer, total);
        if let Some(bits) = peer.parts.as_mut() {
            bits.set_index(i64::from(index), true);
        }
    }

    /// `broadcastNewValidBlockMessage`. Kept until every current peer accepts it.
    fn broadcast_valid_blocks(&mut self, switch: &Switch) {
        let peers: Vec<String> = switch.peers().into_iter().map(|info| info.id).collect();
        if peers.is_empty() {
            return;
        }
        for node in &mut self.nodes {
            let Some(msg) = node.outbound_valid_block().cloned() else {
                continue;
            };
            let bytes = valid_block_message(&msg);
            if peers
                .iter()
                .all(|id| switch.try_send(id, STATE_CHANNEL, &bytes))
            {
                node.clear_outbound_valid_block();
            }
        }
    }

    fn mark_vote(&mut self, peer_id: &str, vote: &Vote) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if let Some(bits) = vote_slot(peer, vote.vote_type, self.validator_count) {
            bits.set_index(i64::from(vote.validator_index), true);
        }
    }

    fn handle(&mut self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let Ok(message) = Message::decode(bytes) else {
            return false;
        };
        let Some(sum) = message.sum else {
            return false;
        };
        self.peers
            .entry(peer_id.to_owned())
            .or_insert_with(PeerState::new);
        match (ch_id, sum) {
            (STATE_CHANNEL, message::Sum::NewRoundStep(msg)) => {
                if let Some(peer) = self.peers.get_mut(peer_id) {
                    peer.apply_round_step(&msg);
                }
            }
            (STATE_CHANNEL, message::Sum::HasVote(msg)) => self.apply_has_vote(peer_id, &msg),
            (STATE_CHANNEL, message::Sum::NewValidBlock(msg)) => {
                self.apply_new_valid_block(peer_id, &msg);
            }
            (STATE_CHANNEL, message::Sum::VoteSetMaj23(_)) => {}
            (DATA_CHANNEL, message::Sum::Proposal(msg)) => {
                if !self.apply_proposal(peer_id, msg) {
                    return false;
                }
            }
            (DATA_CHANNEL, message::Sum::BlockPart(msg)) => {
                if !self.apply_part(peer_id, msg) {
                    return false;
                }
            }
            (DATA_CHANNEL, message::Sum::ProposalPol(_)) => {}
            (VOTE_CHANNEL, message::Sum::Vote(msg)) => {
                if !self.apply_vote(peer_id, msg) {
                    return false;
                }
            }
            (VOTE_SET_BITS_CHANNEL, message::Sum::VoteSetBits(msg)) => {
                self.apply_vote_set_bits(peer_id, &msg);
            }
            _ => {}
        }
        true
    }

    fn apply_proposal(&mut self, peer_id: &str, msg: ProtoProposal) -> bool {
        let Some(proto) = msg.proposal.as_ref() else {
            return false;
        };
        let Ok(proposal) = Proposal::try_from_proto(proto) else {
            return false;
        };
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.note_proposal(&proposal);
        }
        self.remember_proposal(proposal.clone());
        for node in &mut self.nodes {
            node.deliver(Msg::Proposal(proposal.clone()));
        }
        true
    }

    fn apply_part(&mut self, peer_id: &str, msg: BlockPart) -> bool {
        let Ok(part) = Part::try_from_proto(msg.part.as_ref()) else {
            return false;
        };
        let matches_peer = self
            .peers
            .get(peer_id)
            .is_some_and(|peer| peer.height == msg.height && peer.round == msg.round);
        if matches_peer {
            self.mark_part(peer_id, part.index);
        }
        for node in &mut self.nodes {
            if node.height() == msg.height {
                node.deliver(Msg::Part(part.clone()));
            }
        }
        true
    }

    fn apply_vote(&mut self, peer_id: &str, msg: ProtoVote) -> bool {
        let Some(proto) = msg.vote.as_ref() else {
            return false;
        };
        let Ok(vote) = Vote::try_from_proto(proto) else {
            return false;
        };
        self.mark_vote(peer_id, &vote);
        self.remember_vote(vote.clone());
        for node in &mut self.nodes {
            node.deliver(Msg::Vote(vote.clone()));
        }
        true
    }

    /// `ApplyNewValidBlockMessage`.
    fn apply_new_valid_block(&mut self, peer_id: &str, msg: &NewValidBlock) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if peer.height != msg.height {
            return;
        }
        if peer.round != msg.round && !msg.is_commit {
            return;
        }
        let Some(header_proto) = msg.block_part_set_header.as_ref() else {
            return;
        };
        let Ok(header) = PartSetHeader::try_from_proto(header_proto) else {
            return;
        };
        let Ok(Some(bits)) = BitArray::try_from_proto(msg.block_parts.as_ref()) else {
            return;
        };
        peer.proposal_part_set_header = Some(header);
        peer.parts = Some(bits);
    }

    fn apply_has_vote(&mut self, peer_id: &str, msg: &HasVote) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        let Some(kind) = vote_type(msg.r#type) else {
            return;
        };
        if let Some(bits) = vote_slot(peer, kind, self.validator_count) {
            bits.set_index(i64::from(msg.index), true);
        }
    }

    fn apply_vote_set_bits(&mut self, peer_id: &str, msg: &VoteSetBits) {
        let Ok(Some(bits)) = BitArray::try_from_proto(msg.votes.as_ref()) else {
            return;
        };
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        match vote_type(msg.r#type) {
            Some(SignedMsgType::Prevote) => peer.prevotes = Some(bits),
            Some(SignedMsgType::Precommit) => peer.precommits = Some(bits),
            _ => {}
        }
    }
}

fn timeout_key(timeout: &Scheduled) -> (i64, i32, u32, i64) {
    (
        timeout.height,
        timeout.round,
        timeout.step.as_wal(),
        timeout.delay_nanos,
    )
}

fn send_vote(switch: &Switch, peer_id: &str, vote: &Vote) -> bool {
    let bytes = Message {
        sum: Some(message::Sum::Vote(ProtoVote {
            vote: Some(vote.to_proto()),
        })),
    }
    .encode_to_vec();
    switch.try_send(peer_id, VOTE_CHANNEL, &bytes)
}

fn valid_block_message(msg: &OutboundValidBlock) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::NewValidBlock(NewValidBlock {
            height: msg.height,
            round: msg.round,
            block_part_set_header: Some(msg.header.to_proto()),
            block_parts: msg.parts.to_proto(),
            is_commit: msg.is_commit,
        })),
    }
    .encode_to_vec()
}

fn send_block_part(switch: &Switch, peer_id: &str, height: i64, round: i32, part: &Part) -> bool {
    let bytes = Message {
        sum: Some(message::Sum::BlockPart(BlockPart {
            height,
            round,
            part: Some(part.to_proto()),
        })),
    }
    .encode_to_vec();
    switch.try_send(peer_id, DATA_CHANNEL, &bytes)
}

fn round_step_message<E: ExecApp, C: MempoolApp, D: Db>(node: &Node<E, C, D>) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::NewRoundStep(NewRoundStep {
            height: node.height(),
            round: node.round(),
            step: node.step().as_wal(),
            seconds_since_start_time: 0,
            last_commit_round: -1,
        })),
    }
    .encode_to_vec()
}

fn ensure_parts(peer: &mut PeerState, total: u32) {
    let need = i64::from(total.max(1));
    if peer.parts.as_ref().is_none_or(|bits| bits.size() < need) {
        peer.parts = BitArray::new(need);
    }
}

fn vote_bit(peer: &PeerState, vote: &Vote) -> bool {
    let bits = match vote.vote_type {
        SignedMsgType::Prevote => peer.prevotes.as_ref(),
        SignedMsgType::Precommit => peer.precommits.as_ref(),
        _ => None,
    };
    bits.is_some_and(|bits| bits.get_index(i64::from(vote.validator_index)))
}

fn vote_slot(
    peer: &mut PeerState,
    kind: SignedMsgType,
    validator_count: i64,
) -> Option<&mut BitArray> {
    let slot = match kind {
        SignedMsgType::Prevote => &mut peer.prevotes,
        SignedMsgType::Precommit => &mut peer.precommits,
        _ => return None,
    };
    if slot.is_none() {
        *slot = BitArray::new(validator_count.max(1));
    }
    slot.as_mut()
}

fn vote_type(value: i32) -> Option<SignedMsgType> {
    if value == SignedMsgType::Prevote as i32 {
        Some(SignedMsgType::Prevote)
    } else if value == SignedMsgType::Precommit as i32 {
        Some(SignedMsgType::Precommit)
    } else {
        None
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
