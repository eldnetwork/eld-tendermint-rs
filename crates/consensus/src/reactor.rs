//! Consensus gossip on the p2p switch.
//!
//! Four channels carry `tendermint.consensus.Message`. Proposals, block parts, and
//! votes are delivered into the local [`Node`]s. A peer whose height is still in
//! the block store, and behind this node's consensus height, is sent that block's
//! parts on `0x21`. Commit votes go out on `0x22`, one per tick. `VoteSetMaj23`
//! on `0x20` is answered with `VoteSetBits` on `0x23`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eld_tendermint_mempool::App as MempoolApp;
use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::consensus::{
    BlockPart, HasVote, Message, NewRoundStep, NewValidBlock, Proposal as ProtoProposal,
    ProposalPol, Vote as ProtoVote, VoteSetBits, VoteSetMaj23, message,
};
use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};
use eld_tendermint_state::App as ExecApp;
use eld_tendermint_store::{Db, MemDb};
use eld_tendermint_types::{
    BitArray, BlockId, Commit, Level, Part, PartSetHeader, Proposal, Time, Vote, log_line,
};
use prost::Message as ProstMessage;

use crate::round::{CommitSource, Msg, Node, OutboundValidBlock, Scheduled, Step, seconds_since};

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
    proposal_pol_round: i32,
    proposal_pol: Option<BitArray>,
    last_commit_round: i32,
    last_commit: Option<BitArray>,
    catchup_commit_round: i32,
    catchup_commit: Option<BitArray>,
}

impl PeerState {
    fn new() -> Self {
        Self {
            height: 0,
            round: -1,
            step: 0,
            proposal_hash: None,
            has_proposal: false,
            proposal_part_set_header: None,
            parts: None,
            prevotes: None,
            precommits: None,
            proposal_pol_round: -1,
            proposal_pol: None,
            last_commit_round: -1,
            last_commit: None,
            catchup_commit_round: -1,
            catchup_commit: None,
        }
    }

    /// `ApplyNewRoundStepMessage`.
    fn apply_round_step(&mut self, msg: &NewRoundStep) {
        if (msg.height, msg.round, msg.step) <= (self.height, self.round, self.step) {
            return;
        }
        let ps_height = self.height;
        let ps_round = self.round;
        let ps_catchup_commit_round = self.catchup_commit_round;
        let ps_catchup_commit = self.catchup_commit.clone();
        let old_precommits = self.precommits.clone();

        self.height = msg.height;
        self.round = msg.round;
        self.step = msg.step;
        if ps_height != msg.height || ps_round != msg.round {
            self.proposal_hash = None;
            self.has_proposal = false;
            self.proposal_part_set_header = None;
            self.parts = None;
            self.proposal_pol_round = -1;
            self.proposal_pol = None;
            self.prevotes = None;
            self.precommits = None;
        }
        if ps_height == msg.height && ps_round != msg.round && msg.round == ps_catchup_commit_round
        {
            self.precommits = ps_catchup_commit;
        }
        if ps_height != msg.height {
            if ps_height + 1 == msg.height && ps_round == msg.last_commit_round {
                self.last_commit_round = msg.last_commit_round;
                self.last_commit = old_precommits;
            } else {
                self.last_commit_round = msg.last_commit_round;
                self.last_commit = None;
            }
            self.catchup_commit_round = -1;
            self.catchup_commit = None;
        }
    }

    /// `SetHasProposal`. A part set already stored for this height and round stays.
    /// The POL round is taken from the proposal and the POL bits are cleared.
    fn note_proposal(&mut self, proposal: &Proposal) {
        if self.height != proposal.height || self.round != proposal.round || self.has_proposal {
            let peer_height = self.height.to_string();
            let peer_round = self.round.to_string();
            let height = proposal.height.to_string();
            let round = proposal.round.to_string();
            let has_proposal = self.has_proposal.to_string();
            log_line(
                Level::Info,
                "consensus",
                "note proposal skipped",
                &[
                    ("peer_height", peer_height.as_str()),
                    ("peer_round", peer_round.as_str()),
                    ("height", height.as_str()),
                    ("round", round.as_str()),
                    ("has_proposal", has_proposal.as_str()),
                ],
            );
            return;
        }
        self.has_proposal = true;
        self.proposal_hash = Some(proposal.block_id.hash.clone());
        self.proposal_pol_round = proposal.pol_round;
        self.proposal_pol = None;
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

struct Pick {
    height: i64,
    round: i32,
    kind: SignedMsgType,
    is_commit: bool,
    size: i64,
}

/// Height, round, vote type, validator index. Sent once so peers are not flooded.
type HasVoteKey = (i64, i32, i32, i32);

struct Inner<E: ExecApp, C: MempoolApp, D: Db> {
    nodes: Vec<Node<E, C, D>>,
    peers: HashMap<String, PeerState>,
    announced: Vec<Option<(i64, i32, u32)>>,
    proposals: Vec<Proposal>,
    timeouts: Vec<Option<TimeoutWatch>>,
    validator_count: i64,
    /// Local votes waiting for a peer before `HasVote` can go out.
    pending_has_votes: Vec<Vote>,
    /// `HasVote` keys already given to the current peers.
    announced_has_votes: HashSet<HasVoteKey>,
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
                timeouts: vec![None; n],
                validator_count,
                pending_has_votes: Vec::new(),
                announced_has_votes: HashSet::new(),
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
    /// Returns `false` when `bytes` is not a `consensus.Message`, when a part set
    /// grows past `Block.MaxBytes`, or when `VoteSetMaj23` conflicts for this peer.
    /// The caller stops that peer. A bad signature or a bad part proof returns `true`.
    /// `switch` carries the `VoteSetBits` reply.
    pub fn handle(&self, switch: &Switch, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let mut inner = lock(&self.inner);
        inner.handle(switch, peer_id, ch_id, bytes)
    }

    /// Part-set header stored for `peer_id`, if this node has accepted one.
    #[must_use]
    pub fn peer_part_set_header(&self, peer_id: &str) -> Option<PartSetHeader> {
        lock(&self.inner)
            .peers
            .get(peer_id)
            .and_then(|peer| peer.proposal_part_set_header.clone())
    }

    /// POL bits stored for `peer_id` after a `ProposalPOL` at that peer's POL round.
    #[must_use]
    pub fn proposal_pol_bits(&self, peer_id: &str) -> Option<BitArray> {
        lock(&self.inner)
            .peers
            .get(peer_id)
            .and_then(|peer| peer.proposal_pol.clone())
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
            // A vote this node signed is announced even when it is not picked for a peer.
            Msg::Vote(vote) => self.pending_has_votes.push(vote),
            // Parts are read from each node's round state on the next poll.
            Msg::Part(_) => {}
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
        self.flush_has_votes(switch);
        self.broadcast_valid_blocks(switch);
        let peer_ids: Vec<String> = self.peers.keys().cloned().collect();
        for peer_id in peer_ids {
            // Catchup goes out before the live round so a full data queue cannot
            // drop the block the peer is missing.
            self.send_catchup(switch, &peer_id);
            self.send_proposals(switch, &peer_id);
            self.send_parts(switch, &peer_id);
            self.send_votes(switch, &peer_id);
            self.send_maj23(switch, &peer_id);
        }
    }

    /// One missing part of a block the peer is still behind on.
    ///
    /// The block store's base is above 0, the peer height is above 0 and below our
    /// consensus height, and that height is still at or above the store base.
    /// Commit votes are sent from [`Self::send_votes`].
    fn send_catchup(&mut self, switch: &Switch, peer_id: &str) {
        let (peer_height, peer_round, our_height) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            let our_height = self.nodes.iter().map(Node::height).max().unwrap_or(0);
            (peer.height, peer.round, our_height)
        };
        if peer_height <= 0 || peer_height >= our_height {
            return;
        }
        let Some(source) = self.nodes.iter().position(|node| {
            let base = node.store_base();
            base > 0
                && peer_height >= base
                && node.store_height() >= peer_height
                && node.block_part_count(peer_height).is_some()
        }) else {
            return;
        };
        let Some(total) = self.nodes[source].block_part_count(peer_height) else {
            return;
        };
        if total == 0 {
            return;
        }
        let Some(block_header) = self.nodes[source].block_part_set_header(peer_height) else {
            return;
        };
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
        let Some(peer_bits) = self.peers.get(peer_id).and_then(|peer| peer.parts.clone()) else {
            return;
        };
        let Some(index) = peer_bits.not().pick_random() else {
            return;
        };
        let Some(index) = u32::try_from(index).ok() else {
            return;
        };
        let Some(part) = self.nodes[source].block_part(peer_height, index) else {
            return;
        };
        if send_block_part(switch, peer_id, peer_height, peer_round, &part) {
            self.set_has_proposal_block_part(peer_id, peer_height, peer_round, index);
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
                if proposal.pol_round >= 0 {
                    if let Some(bits) = self.prevote_bits(proposal.pol_round) {
                        let _ = switch.try_send(
                            peer_id,
                            DATA_CHANNEL,
                            &proposal_pol_message(proposal.height, proposal.pol_round, &bits),
                        );
                    }
                }
            } else {
                let height = proposal.height.to_string();
                let round = proposal.round.to_string();
                log_line(
                    Level::Info,
                    "consensus",
                    "send proposal failed",
                    &[
                        ("peer", peer_id),
                        ("height", height.as_str()),
                        ("round", round.as_str()),
                    ],
                );
            }
        }
    }

    /// One proposal part whose part-set header matches the peer's.
    ///
    /// Go matches parts on the header hash, so the peer's round can differ.
    /// The message still carries this node's height and round.
    fn send_parts(&mut self, switch: &Switch, peer_id: &str) {
        let (peer_height, peer_round, header, peer_bits) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            let Some(header) = peer.proposal_part_set_header.clone() else {
                log_send_parts(
                    peer_id,
                    peer.height,
                    peer.round,
                    "send parts: no proposal part set header",
                );
                return;
            };
            (peer.height, peer.round, header, peer.parts.clone())
        };
        let mut saw_same_height = false;
        let mut matched_header = false;
        let found = self.nodes.iter().find_map(|node| {
            if node.height() != peer_height {
                return None;
            }
            saw_same_height = true;
            if node.proposal_part_set_header().as_ref() != Some(&header) {
                return None;
            }
            matched_header = true;
            if !node.proposal_parts_has_header(&header) {
                return None;
            }
            let bits = node.proposal_parts_bits()?;
            let missing = BitArray::sub(Some(&bits), peer_bits.as_ref())?;
            let index = missing.pick_random()?;
            let index = u32::try_from(index).ok()?;
            let part = node.proposal_part(index)?;
            Some((node.height(), node.round(), part))
        });
        let Some((height, round, part)) = found else {
            let message = if saw_same_height && !matched_header {
                "send parts: header mismatch"
            } else {
                "send parts: no missing part"
            };
            log_send_parts(peer_id, peer_height, peer_round, message);
            return;
        };
        if send_block_part(switch, peer_id, height, round, &part) {
            self.mark_part(peer_id, part.index);
        }
    }

    /// One vote, the first success in `gossipVotesRoutine` / `gossipVotesForHeight` order.
    fn send_votes(&mut self, switch: &Switch, peer_id: &str) {
        let (peer_height, peer_round, peer_step, pol_round) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            (peer.height, peer.round, peer.step, peer.proposal_pol_round)
        };
        let (our_height, our_round) = self.consensus_hrs();
        if peer_height == our_height {
            if peer_step == Step::NewHeight.as_wal() && self.send_last_commit(switch, peer_id) {
                return;
            }
            if peer_step <= Step::Propose.as_wal()
                && peer_round != -1
                && peer_round <= our_round
                && pol_round != -1
                && self.send_round_votes(switch, peer_id, pol_round, SignedMsgType::Prevote)
            {
                return;
            }
            if peer_step <= Step::PrevoteWait.as_wal()
                && peer_round != -1
                && peer_round <= our_round
                && self.send_round_votes(switch, peer_id, peer_round, SignedMsgType::Prevote)
            {
                return;
            }
            if peer_step <= Step::PrecommitWait.as_wal()
                && peer_round != -1
                && peer_round <= our_round
                && self.send_round_votes(switch, peer_id, peer_round, SignedMsgType::Precommit)
            {
                return;
            }
            if peer_round != -1
                && peer_round <= our_round
                && self.send_round_votes(switch, peer_id, peer_round, SignedMsgType::Prevote)
            {
                return;
            }
            if pol_round != -1
                && self.send_round_votes(switch, peer_id, pol_round, SignedMsgType::Prevote)
            {
                return;
            }
        }
        if peer_height != 0
            && our_height == peer_height + 1
            && self.send_last_commit(switch, peer_id)
        {
            return;
        }
        if peer_height != 0 && our_height >= peer_height + 2 {
            let _ = self.send_block_commit(switch, peer_id, peer_height);
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

    fn handle(&mut self, switch: &Switch, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
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
            (STATE_CHANNEL, message::Sum::VoteSetMaj23(msg)) => {
                if !self.apply_vote_set_maj23(switch, peer_id, &msg) {
                    return false;
                }
            }
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
            (DATA_CHANNEL, message::Sum::ProposalPol(msg)) => {
                self.apply_proposal_pol(peer_id, &msg);
            }
            (VOTE_CHANNEL, message::Sum::Vote(msg)) => {
                if !self.apply_vote(switch, peer_id, msg) {
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
        if proposal.pol_round < -1 || proposal.pol_round >= proposal.round {
            return false;
        }
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.note_proposal(&proposal);
        }
        self.remember_proposal(proposal.clone());
        let mut accepted = true;
        for node in &mut self.nodes {
            if !node.deliver(Msg::Proposal(proposal.clone())) {
                accepted = false;
            }
        }
        accepted
    }

    fn apply_part(&mut self, peer_id: &str, msg: BlockPart) -> bool {
        let Ok(part) = Part::try_from_proto(msg.part.as_ref()) else {
            return false;
        };
        let matches_peer = self
            .peers
            .get(peer_id)
            .is_some_and(|peer| peer.height == msg.height && peer.round == msg.round);
        let height = msg.height.to_string();
        let round = msg.round.to_string();
        let index = part.index.to_string();
        let matches = matches_peer.to_string();
        log_line(
            Level::Info,
            "consensus",
            "apply part",
            &[
                ("peer", peer_id),
                ("height", height.as_str()),
                ("round", round.as_str()),
                ("index", index.as_str()),
                ("matches_peer", matches.as_str()),
            ],
        );
        if matches_peer {
            self.mark_part(peer_id, part.index);
        }
        let mut ok = true;
        for node in &mut self.nodes {
            if node.height() == msg.height && !node.deliver(Msg::Part(part.clone())) {
                ok = false;
            }
        }
        ok
    }

    fn apply_vote(&mut self, switch: &Switch, peer_id: &str, msg: ProtoVote) -> bool {
        let Some(proto) = msg.vote.as_ref() else {
            return false;
        };
        let Ok(vote) = Vote::try_from_proto(proto) else {
            return false;
        };
        self.note_incoming_vote(peer_id, &vote);
        for node in &mut self.nodes {
            node.deliver(Msg::Vote(vote.clone()));
        }
        if self.nodes.iter().any(|node| holds_vote(node, &vote))
            && !self.broadcast_has_vote(switch, &vote)
        {
            self.pending_has_votes.push(vote);
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
        if peer.height != msg.height {
            return;
        }
        let Some(kind) = vote_type(msg.r#type) else {
            return;
        };
        if let Some(bits) = vote_bits(peer, msg.height, msg.round, kind) {
            bits.set_index(i64::from(msg.index), true);
        }
    }

    /// `ApplyVoteSetBitsMessage`.
    fn apply_vote_set_bits(&mut self, peer_id: &str, msg: &VoteSetBits) {
        let Ok(Some(msg_bits)) = BitArray::try_from_proto(msg.votes.as_ref()) else {
            return;
        };
        let Some(kind) = vote_type(msg.r#type) else {
            return;
        };
        let our_votes = if self.nodes.iter().any(|node| node.height() == msg.height) {
            self.bits_for_block(msg.round, kind, msg.block_id.as_ref())
        } else {
            None
        };
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        let Some(votes) = vote_bits(peer, msg.height, msg.round, kind) else {
            return;
        };
        if let Some(ours) = our_votes.as_ref() {
            let other = BitArray::sub(Some(votes), Some(ours));
            let has = BitArray::or(other.as_ref(), Some(&msg_bits));
            votes.update(has.as_ref());
        } else {
            votes.update(Some(&msg_bits));
        }
    }

    /// `ApplyProposalPOLMessage`. Bits are replaced only when the height and POL round match.
    fn apply_proposal_pol(&mut self, peer_id: &str, msg: &ProposalPol) {
        let Ok(bits) = BitArray::try_from_proto(msg.proposal_pol.as_ref()) else {
            return;
        };
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if peer.height != msg.height || peer.proposal_pol_round != msg.proposal_pol_round {
            return;
        }
        peer.proposal_pol = bits;
    }

    /// `VoteSetMaj23` receive. A conflicting claim from this peer returns `false`.
    fn apply_vote_set_maj23(&mut self, switch: &Switch, peer_id: &str, msg: &VoteSetMaj23) -> bool {
        let Some(kind) = vote_type(msg.r#type) else {
            return false;
        };
        let Some(block_proto) = msg.block_id.as_ref() else {
            return false;
        };
        let Ok(block_id) = BlockId::try_from_proto(block_proto) else {
            return false;
        };
        if !self.nodes.iter().any(|node| node.height() == msg.height) {
            return true;
        }
        let mut conflict = false;
        for node in &mut self.nodes {
            if node.height() != msg.height {
                continue;
            }
            if node
                .set_peer_maj23(msg.round, kind, peer_id, &block_id)
                .is_err()
            {
                conflict = true;
            }
        }
        if conflict {
            return false;
        }
        let bits = self.bits_for_block(msg.round, kind, Some(block_proto));
        let _ = switch.try_send(
            peer_id,
            VOTE_SET_BITS_CHANNEL,
            &vote_set_bits_message(msg.height, msg.round, kind, &block_id, bits.as_ref()),
        );
        true
    }

    fn send_maj23(&mut self, switch: &Switch, peer_id: &str) {
        let (peer_height, peer_round, pol_round, catchup_round) = {
            let Some(peer) = self.peers.get(peer_id) else {
                return;
            };
            (
                peer.height,
                peer.round,
                peer.proposal_pol_round,
                peer.catchup_commit_round,
            )
        };
        let (our_height, _) = self.consensus_hrs();
        if peer_height == our_height {
            if let Some(block_id) = self.maj23(peer_round, SignedMsgType::Prevote) {
                let _ = switch.try_send(
                    peer_id,
                    STATE_CHANNEL,
                    &maj23_message(peer_height, peer_round, SignedMsgType::Prevote, &block_id),
                );
            }
            if let Some(block_id) = self.maj23(peer_round, SignedMsgType::Precommit) {
                let _ = switch.try_send(
                    peer_id,
                    STATE_CHANNEL,
                    &maj23_message(peer_height, peer_round, SignedMsgType::Precommit, &block_id),
                );
            }
            if pol_round >= 0 {
                if let Some(block_id) = self.maj23(pol_round, SignedMsgType::Prevote) {
                    let _ = switch.try_send(
                        peer_id,
                        STATE_CHANNEL,
                        &maj23_message(peer_height, pol_round, SignedMsgType::Prevote, &block_id),
                    );
                }
            }
        }
        if catchup_round != -1 && peer_height > 0 {
            if let Some((round, block_id)) = self.block_commit_id(peer_height) {
                let _ = switch.try_send(
                    peer_id,
                    STATE_CHANNEL,
                    &maj23_message(peer_height, round, SignedMsgType::Precommit, &block_id),
                );
            }
        }
    }

    fn consensus_hrs(&self) -> (i64, i32) {
        let height = self.nodes.iter().map(Node::height).max().unwrap_or(0);
        let round = self
            .nodes
            .iter()
            .filter(|node| node.height() == height)
            .map(Node::round)
            .max()
            .unwrap_or(0);
        (height, round)
    }

    fn prevote_bits(&self, round: i32) -> Option<BitArray> {
        or_bits(
            self.nodes
                .iter()
                .filter_map(|node| node.prevote_bits(round)),
        )
    }

    fn maj23(&self, round: i32, kind: SignedMsgType) -> Option<BlockId> {
        self.nodes.iter().find_map(|node| match kind {
            SignedMsgType::Prevote => node.prevote_maj23(round),
            SignedMsgType::Precommit => node.precommit_maj23(round),
            _ => None,
        })
    }

    fn bits_for_block(
        &self,
        round: i32,
        kind: SignedMsgType,
        block_id: Option<&eld_tendermint_proto::types::BlockId>,
    ) -> Option<BitArray> {
        let block_id = BlockId::try_from_proto(block_id?).ok()?;
        or_bits(self.nodes.iter().filter_map(|node| match kind {
            SignedMsgType::Prevote => node.prevote_bits_by_block_id(round, &block_id),
            SignedMsgType::Precommit => node.precommit_bits_by_block_id(round, &block_id),
            _ => None,
        }))
    }

    fn send_round_votes(
        &mut self,
        switch: &Switch,
        peer_id: &str,
        round: i32,
        kind: SignedMsgType,
    ) -> bool {
        let (bits, size) = match kind {
            SignedMsgType::Prevote => (
                or_bits(
                    self.nodes
                        .iter()
                        .filter_map(|node| node.prevote_bits(round)),
                ),
                self.validator_count,
            ),
            SignedMsgType::Precommit => (
                or_bits(
                    self.nodes
                        .iter()
                        .filter_map(|node| node.precommit_bits(round)),
                ),
                self.validator_count,
            ),
            _ => return false,
        };
        let Some(bits) = bits else {
            return false;
        };
        let Some(height) = self.nodes.iter().find_map(|node| {
            let present = match kind {
                SignedMsgType::Prevote => node.prevote_bits(round).is_some(),
                SignedMsgType::Precommit => node.precommit_bits(round).is_some(),
                _ => false,
            };
            present.then(|| node.height())
        }) else {
            return false;
        };
        let Some(index) = self.pick_index(
            peer_id,
            &bits,
            Pick {
                height,
                round,
                kind,
                is_commit: false,
                size,
            },
        ) else {
            return false;
        };
        let vote = self.nodes.iter().find_map(|node| match kind {
            SignedMsgType::Prevote => node.prevote_at(round, index),
            SignedMsgType::Precommit => node.precommit_at(round, index),
            _ => None,
        });
        self.send_picked_vote(switch, peer_id, vote)
    }

    fn send_last_commit(&mut self, switch: &Switch, peer_id: &str) -> bool {
        let Some(source) = self.last_commit_source() else {
            return false;
        };
        let Some(index) = self.pick_index(
            peer_id,
            &source.bits,
            Pick {
                height: source.height,
                round: source.round,
                kind: SignedMsgType::Precommit,
                is_commit: source.is_commit,
                size: source.size,
            },
        ) else {
            return false;
        };
        let vote = self
            .nodes
            .iter()
            .find_map(|node| node.last_commit_vote(index));
        self.send_picked_vote(switch, peer_id, vote)
    }

    fn send_block_commit(&mut self, switch: &Switch, peer_id: &str, height: i64) -> bool {
        let Some(commit) = self.nodes.iter().find_map(|node| node.block_commit(height)) else {
            return false;
        };
        if !self.nodes.iter().any(|node| {
            let base = node.store_base();
            base > 0 && height >= base && node.store_height() >= height
        }) {
            return false;
        }
        let Some(bits) = commit_bit_array(&commit) else {
            return false;
        };
        let size = i64::try_from(commit.signatures.len()).unwrap_or(0);
        let Some(index) = self.pick_index(
            peer_id,
            &bits,
            Pick {
                height: commit.height,
                round: commit.round,
                kind: SignedMsgType::Precommit,
                is_commit: !commit.signatures.is_empty(),
                size,
            },
        ) else {
            return false;
        };
        self.send_picked_vote(switch, peer_id, commit_vote(&commit, index))
    }

    fn last_commit_source(&self) -> Option<CommitSource> {
        let mut combined: Option<CommitSource> = None;
        for node in &self.nodes {
            let Some(source) = node.last_commit_source() else {
                continue;
            };
            combined = Some(match combined {
                None => source,
                Some(mut have) => {
                    if let Some(bits) = BitArray::or(Some(&have.bits), Some(&source.bits)) {
                        have.bits = bits;
                    }
                    have.is_commit = have.is_commit || source.is_commit;
                    have
                }
            });
        }
        combined
    }

    fn block_commit_id(&self, height: i64) -> Option<(i32, BlockId)> {
        let commit = self
            .nodes
            .iter()
            .find_map(|node| node.block_commit(height))?;
        if !self.nodes.iter().any(|node| {
            let base = node.store_base();
            base > 0 && height >= base && node.store_height() >= height
        }) {
            return None;
        }
        Some((commit.round, commit.block_id))
    }

    fn pick_index(&mut self, peer_id: &str, our_bits: &BitArray, pick: Pick) -> Option<i32> {
        if pick.size == 0 {
            return None;
        }
        let peer = self.peers.get_mut(peer_id)?;
        if pick.is_commit {
            ensure_catchup_commit_round(peer, pick.height, pick.round, pick.size);
        }
        ensure_vote_bit_arrays(peer, pick.height, pick.size);
        let peer_bits = vote_bits(peer, pick.height, pick.round, pick.kind)?.copy();
        let missing = BitArray::sub(Some(our_bits), Some(&peer_bits))?;
        let index = missing.pick_random()?;
        i32::try_from(index).ok()
    }

    fn send_picked_vote(&mut self, switch: &Switch, peer_id: &str, vote: Option<Vote>) -> bool {
        let Some(vote) = vote else {
            return false;
        };
        if !send_vote(switch, peer_id, &vote) {
            return false;
        }
        self.note_sent_vote(peer_id, &vote);
        if !self.broadcast_has_vote(switch, &vote) {
            self.pending_has_votes.push(vote);
        }
        true
    }

    fn note_incoming_vote(&mut self, peer_id: &str, vote: &Vote) {
        let size = self.validator_count.max(1);
        let (our_height, _) = self.consensus_hrs();
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        ensure_vote_bit_arrays(peer, our_height, size);
        ensure_vote_bit_arrays(peer, our_height - 1, size);
        if let Some(bits) = vote_bits(peer, vote.height, vote.round, vote.vote_type) {
            bits.set_index(i64::from(vote.validator_index), true);
        }
    }

    fn note_sent_vote(&mut self, peer_id: &str, vote: &Vote) {
        let Some(peer) = self.peers.get_mut(peer_id) else {
            return;
        };
        if let Some(bits) = vote_bits(peer, vote.height, vote.round, vote.vote_type) {
            bits.set_index(i64::from(vote.validator_index), true);
        }
    }

    /// `broadcastHasVoteMessage`. One `HasVote` per vote, on `0x20`, to every peer.
    ///
    /// Returns false when there is no peer or every send fails, so the caller can retry.
    fn broadcast_has_vote(&mut self, switch: &Switch, vote: &Vote) -> bool {
        let peer_ids: Vec<String> = switch.peers().into_iter().map(|info| info.id).collect();
        if peer_ids.is_empty() {
            return false;
        }
        let key = has_vote_key(vote);
        if self.announced_has_votes.contains(&key) {
            return true;
        }
        let bytes = has_vote_message(vote);
        let mut sent = false;
        for id in &peer_ids {
            if switch.try_send(id, STATE_CHANNEL, &bytes) {
                sent = true;
            }
        }
        if sent {
            self.announced_has_votes.insert(key);
        }
        sent
    }

    fn flush_has_votes(&mut self, switch: &Switch) {
        if switch.peers().is_empty() {
            return;
        }
        let votes = std::mem::take(&mut self.pending_has_votes);
        for vote in votes {
            if !self.broadcast_has_vote(switch, &vote) {
                self.pending_has_votes.push(vote);
            }
        }
    }
}

fn has_vote_key(vote: &Vote) -> HasVoteKey {
    (
        vote.height,
        vote.round,
        vote.vote_type as i32,
        vote.validator_index,
    )
}

fn holds_vote<E: ExecApp, C: MempoolApp, D: Db>(node: &Node<E, C, D>, vote: &Vote) -> bool {
    let stored = match vote.vote_type {
        SignedMsgType::Prevote => node.prevote_at(vote.round, vote.validator_index),
        SignedMsgType::Precommit => node.precommit_at(vote.round, vote.validator_index),
        _ => None,
    };
    stored.is_some_and(|have| have.signature == vote.signature)
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

fn log_send_parts(peer_id: &str, height: i64, round: i32, message: &str) {
    let height = height.to_string();
    let round = round.to_string();
    log_line(
        Level::Info,
        "consensus",
        message,
        &[
            ("peer", peer_id),
            ("height", height.as_str()),
            ("round", round.as_str()),
        ],
    );
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
            seconds_since_start_time: seconds_since(node.round_start(), Time::now()),
            last_commit_round: node.last_commit_round(),
        })),
    }
    .encode_to_vec()
}

fn has_vote_message(vote: &Vote) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::HasVote(HasVote {
            height: vote.height,
            round: vote.round,
            r#type: vote.vote_type as i32,
            index: vote.validator_index,
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

/// `getVoteBitArray`.
fn vote_bits(
    peer: &mut PeerState,
    height: i64,
    round: i32,
    kind: SignedMsgType,
) -> Option<&mut BitArray> {
    if kind != SignedMsgType::Prevote && kind != SignedMsgType::Precommit {
        return None;
    }
    if peer.height == height {
        if peer.round == round {
            return match kind {
                SignedMsgType::Prevote => peer.prevotes.as_mut(),
                SignedMsgType::Precommit => peer.precommits.as_mut(),
                _ => None,
            };
        }
        if peer.catchup_commit_round == round {
            return match kind {
                SignedMsgType::Precommit => peer.catchup_commit.as_mut(),
                _ => None,
            };
        }
        if peer.proposal_pol_round == round {
            return match kind {
                SignedMsgType::Prevote => peer.proposal_pol.as_mut(),
                _ => None,
            };
        }
        return None;
    }
    if peer.height == height + 1 && peer.last_commit_round == round {
        return match kind {
            SignedMsgType::Precommit => peer.last_commit.as_mut(),
            _ => None,
        };
    }
    None
}

fn ensure_vote_bit_arrays(peer: &mut PeerState, height: i64, num_validators: i64) {
    if num_validators <= 0 {
        return;
    }
    if peer.height == height {
        if peer.prevotes.is_none() {
            peer.prevotes = BitArray::new(num_validators);
        }
        if peer.precommits.is_none() {
            peer.precommits = BitArray::new(num_validators);
        }
        if peer.catchup_commit.is_none() {
            peer.catchup_commit = BitArray::new(num_validators);
        }
        if peer.proposal_pol.is_none() {
            peer.proposal_pol = BitArray::new(num_validators);
        }
    } else if peer.height == height + 1 && peer.last_commit.is_none() {
        peer.last_commit = BitArray::new(num_validators);
    }
}

fn ensure_catchup_commit_round(peer: &mut PeerState, height: i64, round: i32, num_validators: i64) {
    if peer.height != height || peer.catchup_commit_round == round {
        return;
    }
    peer.catchup_commit_round = round;
    if round == peer.round {
        peer.catchup_commit = peer.precommits.clone();
    } else {
        peer.catchup_commit = BitArray::new(num_validators);
    }
}

fn or_bits(bits: impl Iterator<Item = BitArray>) -> Option<BitArray> {
    let mut combined = None;
    for bits in bits {
        combined = BitArray::or(combined.as_ref(), Some(&bits));
    }
    combined
}

fn proposal_pol_message(height: i64, pol_round: i32, bits: &BitArray) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::ProposalPol(ProposalPol {
            height,
            proposal_pol_round: pol_round,
            proposal_pol: bits.to_proto(),
        })),
    }
    .encode_to_vec()
}

fn maj23_message(height: i64, round: i32, kind: SignedMsgType, block_id: &BlockId) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::VoteSetMaj23(VoteSetMaj23 {
            height,
            round,
            r#type: kind as i32,
            block_id: Some(block_id.to_proto()),
        })),
    }
    .encode_to_vec()
}

fn vote_set_bits_message(
    height: i64,
    round: i32,
    kind: SignedMsgType,
    block_id: &BlockId,
    bits: Option<&BitArray>,
) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::VoteSetBits(VoteSetBits {
            height,
            round,
            r#type: kind as i32,
            block_id: Some(block_id.to_proto()),
            votes: bits.and_then(BitArray::to_proto),
        })),
    }
    .encode_to_vec()
}

fn commit_bit_array(commit: &Commit) -> Option<BitArray> {
    let mut bits = BitArray::new(i64::try_from(commit.signatures.len()).unwrap_or(0))?;
    for (index, sig) in commit.signatures.iter().enumerate() {
        if !sig.is_absent() {
            bits.set_index(i64::try_from(index).unwrap_or(0), true);
        }
    }
    Some(bits)
}

fn commit_vote(commit: &Commit, index: i32) -> Option<Vote> {
    let sig = commit.signatures.get(usize::try_from(index).ok()?)?;
    let block_id = if sig.block_id_flag == BlockIdFlag::Nil {
        BlockId::default()
    } else {
        commit.block_id.clone()
    };
    Some(Vote {
        vote_type: SignedMsgType::Precommit,
        height: commit.height,
        round: commit.round,
        block_id,
        timestamp: sig.timestamp,
        validator_address: sig.validator_address.clone(),
        validator_index: index,
        signature: sig.signature.clone(),
    })
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
