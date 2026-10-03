//! One validator's round state. No network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eld_tendermint_config::ConsensusConfig;
use eld_tendermint_evidence::ProposalEvidence;
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::{PrivValidator, STEP_PRECOMMIT, STEP_PREVOTE};
use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};
use eld_tendermint_state::{
    App as ExecApp, CommitEvents, IndexTxs, State as ChainState, apply_block,
};
use eld_tendermint_store::{BlockStore, Db, MemDb};
use eld_tendermint_types::{
    BLOCK_PART_SIZE_BYTES, Block, BlockId, Commit, EvidenceList, Level, Part, PartSet, Proposal,
    Time, ValidatorSet, Vote, log_line, upper_hex,
};
use prost::Message;

use crate::error::Error;
use crate::votes::HeightVoteSet;
use crate::wal::{self, Replay, Wal};

/// Round step. Ordered so later steps compare greater, matching the Go checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Step {
    NewHeight,
    NewRound,
    Propose,
    Prevote,
    PrevoteWait,
    Precommit,
    PrecommitWait,
    Commit,
}

impl Step {
    /// Go `RoundStepType`. `NewHeight` is 1.
    #[must_use]
    pub fn round_step(self) -> u32 {
        self.as_wal()
    }

    /// Go `RoundStepType`. NewHeight is 1.
    #[must_use]
    pub(crate) fn as_wal(self) -> u32 {
        match self {
            Self::NewHeight => 1,
            Self::NewRound => 2,
            Self::Propose => 3,
            Self::Prevote => 4,
            Self::PrevoteWait => 5,
            Self::Precommit => 6,
            Self::PrecommitWait => 7,
            Self::Commit => 8,
        }
    }

    #[must_use]
    fn from_wal(step: u32) -> Option<Self> {
        match step {
            1 => Some(Self::NewHeight),
            2 => Some(Self::NewRound),
            3 => Some(Self::Propose),
            4 => Some(Self::Prevote),
            5 => Some(Self::PrevoteWait),
            6 => Some(Self::Precommit),
            7 => Some(Self::PrecommitWait),
            8 => Some(Self::Commit),
            _ => None,
        }
    }

    fn log_name(self) -> &'static str {
        match self {
            Self::NewHeight => "RoundStepNewHeight",
            Self::NewRound => "RoundStepNewRound",
            Self::Propose => "RoundStepPropose",
            Self::Prevote => "RoundStepPrevote",
            Self::PrevoteWait => "RoundStepPrevoteWait",
            Self::Precommit => "RoundStepPrecommit",
            Self::PrecommitWait => "RoundStepPrecommitWait",
            Self::Commit => "RoundStepCommit",
        }
    }
}

/// Optional WAL file, evidence pool, tx index, and commit events.
pub struct NodeExtras {
    /// Replay this file before round 0 when it is set.
    pub wal_path: Option<PathBuf>,
    /// Pending evidence included in each proposal.
    pub evidence: Option<Arc<dyn ProposalEvidence>>,
    /// Index DeliverTx results after each saved block.
    pub tx_index: Option<Arc<dyn IndexTxs>>,
    /// Publish `NewBlock` and `Tx` after each saved block.
    pub events: Option<Arc<dyn CommitEvents>>,
}

/// What a validator broadcasts. The group delivers these by method call.
#[derive(Clone, Debug)]
pub enum Msg {
    Proposal(Proposal),
    Part(Part),
    Vote(Vote),
}

#[derive(Clone, Debug)]
pub(crate) struct Scheduled {
    pub delay_nanos: i64,
    pub height: i64,
    pub round: i32,
    pub step: Step,
}

/// In-process consensus state for one validator.
pub struct Node<E: ExecApp, C: MempoolApp, D: Db = MemDb> {
    config: ConsensusConfig,
    pv: Box<dyn PrivValidator>,
    chain_state: ChainState,
    validators: ValidatorSet,
    height: i64,
    round: i32,
    step: Step,
    mempool: Arc<Mutex<Mempool<C>>>,
    block_store: Arc<BlockStore<D>>,
    evidence: Option<Arc<dyn ProposalEvidence>>,
    tx_index: Option<Arc<dyn IndexTxs>>,
    events: Option<Arc<dyn CommitEvents>>,
    exec: E,
    votes: HeightVoteSet,
    proposal: Option<Proposal>,
    proposal_block: Option<Block>,
    proposal_parts: Option<PartSet>,
    locked_block: Option<Block>,
    locked_parts: Option<PartSet>,
    valid_block: Option<Block>,
    valid_parts: Option<PartSet>,
    valid_round: i32,
    last_commit: Option<eld_tendermint_types::VoteSet>,
    reap_count: u32,
    outbox: Vec<Msg>,
    pub(crate) timeout: Option<Scheduled>,
    wal: Option<Wal>,
    replaying: bool,
    /// Round of the +2/3 precommits while `step` is [`Step::Commit`].
    commit_round: Option<i32>,
    /// Parts that arrived before the commit block's part-set header.
    pending_parts: Vec<Part>,
}

impl<E: ExecApp, C: MempoolApp> Node<E, C, MemDb> {
    /// `NewNode` with an in-memory block store.
    ///
    /// # Errors
    ///
    /// Returns a mempool or validator-set error.
    pub fn start(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            Arc::new(BlockStore::new(MemDb::new())),
            NodeExtras {
                wal_path: None,
                evidence: None,
                tx_index: None,
                events: None,
            },
        )
    }

    /// [`Self::start`] with pending evidence included in each proposal.
    ///
    /// # Errors
    ///
    /// Returns a mempool or validator-set error.
    pub fn start_with_evidence(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        evidence: Arc<dyn ProposalEvidence>,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            Arc::new(BlockStore::new(MemDb::new())),
            NodeExtras {
                wal_path: None,
                evidence: Some(evidence),
                tx_index: None,
                events: None,
            },
        )
    }

    /// [`Self::start`], then replay `wal_path` before round 0 when the file has records.
    ///
    /// # Errors
    ///
    /// Returns a WAL, replay, or validator-set error.
    pub fn start_with_wal(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        wal_path: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            Arc::new(BlockStore::new(MemDb::new())),
            NodeExtras {
                wal_path: Some(wal_path.as_ref().to_path_buf()),
                evidence: None,
                tx_index: None,
                events: None,
            },
        )
    }
}

impl<E: ExecApp, C: MempoolApp, D: Db> Node<E, C, D> {
    /// [`Self::start`] saving blocks into `block_store`.
    ///
    /// # Errors
    ///
    /// Returns a mempool or validator-set error.
    pub fn start_with_store(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        block_store: Arc<BlockStore<D>>,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            block_store,
            NodeExtras {
                wal_path: None,
                evidence: None,
                tx_index: None,
                events: None,
            },
        )
    }

    /// [`Self::start_with_store`] with an optional WAL and evidence pool.
    ///
    /// # Errors
    ///
    /// Returns a WAL, replay, mempool, or validator-set error.
    pub fn start_with_store_extras(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        block_store: Arc<BlockStore<D>>,
        extras: NodeExtras,
    ) -> Result<Self, Error> {
        Self::boot(config, pv, chain_state, mempool, exec, block_store, extras)
    }

    /// [`Self::start_with_store`] with pending evidence included in each proposal.
    ///
    /// # Errors
    ///
    /// Returns a mempool or validator-set error.
    pub fn start_with_store_and_evidence(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        block_store: Arc<BlockStore<D>>,
        evidence: Arc<dyn ProposalEvidence>,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            block_store,
            NodeExtras {
                wal_path: None,
                evidence: Some(evidence),
                tx_index: None,
                events: None,
            },
        )
    }

    /// [`Self::start_with_store`], then replay `wal_path` before round 0.
    ///
    /// # Errors
    ///
    /// Returns a WAL, replay, or validator-set error.
    pub fn start_with_wal_and_store(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        block_store: Arc<BlockStore<D>>,
        wal_path: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        Self::boot(
            config,
            pv,
            chain_state,
            mempool,
            exec,
            block_store,
            NodeExtras {
                wal_path: Some(wal_path.as_ref().to_path_buf()),
                evidence: None,
                tx_index: None,
                events: None,
            },
        )
    }
}

impl<E: ExecApp, C: MempoolApp, D: Db> Node<E, C, D> {
    /// The pool this node reaps. The mempool reactor must lock this same handle.
    #[must_use]
    pub fn mempool(&self) -> Arc<Mutex<Mempool<C>>> {
        Arc::clone(&self.mempool)
    }

    fn boot(
        config: ConsensusConfig,
        pv: impl PrivValidator + 'static,
        chain_state: ChainState,
        mempool: Mempool<C>,
        exec: E,
        block_store: Arc<BlockStore<D>>,
        extras: NodeExtras,
    ) -> Result<Self, Error> {
        let height = chain_state.last_block_height + 1;
        let validators = chain_state.validators.copy();
        let votes = HeightVoteSet::new(chain_state.chain_id.as_str(), height, validators.copy());
        let mempool = Arc::new(Mutex::new(mempool));
        let wal = match extras.wal_path {
            Some(path) => Some(Wal::open(path)?),
            None => None,
        };
        let mut node = Self {
            config,
            pv: Box::new(pv),
            chain_state,
            validators,
            height,
            round: 0,
            step: Step::NewHeight,
            mempool,
            block_store,
            evidence: extras.evidence,
            tx_index: extras.tx_index,
            events: extras.events,
            exec,
            votes,
            proposal: None,
            proposal_block: None,
            proposal_parts: None,
            locked_block: None,
            locked_parts: None,
            valid_block: None,
            valid_parts: None,
            valid_round: -1,
            last_commit: None,
            reap_count: 0,
            outbox: Vec::new(),
            timeout: None,
            wal,
            replaying: false,
            commit_round: None,
            pending_parts: Vec::new(),
        };
        if node.wal.is_some() {
            node.catchup()?;
        }
        if node.step == Step::NewHeight {
            let height = node.height;
            node.enter_new_round(height, 0);
        }
        Ok(node)
    }

    /// Signature bytes `FilePV` stored for the last height, round, and step.
    #[must_use]
    pub fn last_signature(&self) -> Option<Vec<u8>> {
        self.pv.last_sign_state().signature.clone()
    }

    #[must_use]
    pub fn step(&self) -> Step {
        self.step
    }

    /// Consensus height. This is the height being decided, not [`Self::store_height`].
    #[must_use]
    pub fn height(&self) -> i64 {
        self.height
    }

    /// Validators in the set this node is using for the current height.
    #[must_use]
    pub fn validator_count(&self) -> usize {
        self.validators.validators().len()
    }

    /// Apply one gossip message. A bad signature or a bad part proof returns without voting.
    pub(crate) fn deliver(&mut self, msg: Msg) {
        match msg {
            Msg::Proposal(proposal) => self.on_proposal(proposal),
            Msg::Part(part) => self.on_part(part),
            Msg::Vote(vote) => self.on_vote(vote),
        }
    }

    #[must_use]
    pub fn round(&self) -> i32 {
        self.round
    }

    #[must_use]
    pub fn store_height(&self) -> i64 {
        self.block_store.height()
    }

    /// One part of a saved block. `None` when that part was not stored.
    #[must_use]
    pub(crate) fn block_part(&self, height: i64, index: u32) -> Option<Part> {
        self.block_store.load_block_part(height, index)
    }

    /// Part-set size of a saved block.
    #[must_use]
    pub(crate) fn block_part_count(&self, height: i64) -> Option<u32> {
        self.block_store
            .load_block_meta(height)
            .map(|meta| meta.block_id.part_set_header.total)
    }

    /// Precommits from the seen commit saved with `height`.
    ///
    /// Signers are [`ChainState::last_validators`], which matches the set that
    /// signed the block when the validator set has not changed. Absent signatures
    /// are skipped.
    #[must_use]
    pub(crate) fn seen_commit_votes(&self, height: i64) -> Vec<Vote> {
        let Some(commit) = self.block_store.load_seen_commit(height) else {
            return Vec::new();
        };
        let validators = self.chain_state.last_validators.validators();
        commit
            .signatures
            .iter()
            .enumerate()
            .filter_map(|(index, sig)| {
                if sig.is_absent() {
                    return None;
                }
                let validator = validators.get(index)?;
                if validator.address != sig.validator_address {
                    return None;
                }
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
                    validator_index: i32::try_from(index).unwrap_or(i32::MAX),
                    signature: sig.signature.clone(),
                })
            })
            .collect()
    }

    #[must_use]
    pub fn committed_hash(&self, height: i64) -> Option<Vec<u8>> {
        self.block_store
            .load_block(height)
            .and_then(|block| block.header.hash())
            .map(|hash| hash.as_bytes().to_vec())
    }

    /// Block saved at `height`, including its evidence list.
    #[must_use]
    pub fn committed_block(&self, height: i64) -> Option<Block> {
        self.block_store.load_block(height)
    }

    /// Block this validator has built for the current round and not yet replaced.
    #[must_use]
    pub fn proposal_block(&self) -> Option<Block> {
        self.proposal_block.clone()
    }

    #[must_use]
    pub fn reap_count(&self) -> u32 {
        self.reap_count
    }

    #[must_use]
    pub fn address(&self) -> Vec<u8> {
        self.pv.get_pub_key().address().to_vec()
    }

    #[must_use]
    pub fn prevote_count(&self, round: i32) -> usize {
        self.votes
            .prevotes(round)
            .map(eld_tendermint_types::VoteSet::vote_count)
            .unwrap_or(0)
    }

    #[must_use]
    pub fn proposal_hash(&self) -> Option<Vec<u8>> {
        self.proposal
            .as_ref()
            .map(|proposal| proposal.block_id.hash.clone())
    }

    /// Proposal waiting to be delivered, if this validator just signed one.
    #[must_use]
    pub fn queued_proposal(&self) -> Option<Proposal> {
        self.outbox.iter().find_map(|msg| match msg {
            Msg::Proposal(proposal) => Some(proposal.clone()),
            _ => None,
        })
    }

    /// Address of the proposer after `extra` further priority increments.
    #[must_use]
    pub fn proposer_after(&self, extra: i32) -> Option<Vec<u8>> {
        let mut validators = self.validators.copy();
        if extra > 0 && validators.increment_proposer_priority(extra).is_err() {
            return None;
        }
        validators
            .proposer()
            .map(|proposer| proposer.address.clone())
    }

    pub(crate) fn return_outbox(&mut self, msgs: Vec<Msg>) {
        let mut merged = msgs;
        merged.append(&mut self.outbox);
        self.outbox = merged;
    }

    pub(crate) fn take_outbox(&mut self) -> Vec<Msg> {
        std::mem::take(&mut self.outbox)
    }

    pub fn on_proposal(&mut self, proposal: Proposal) {
        if self
            .proposal
            .as_ref()
            .is_some_and(|have| have.signature == proposal.signature)
        {
            return;
        }
        if proposal.height != self.height || proposal.round != self.round {
            return;
        }
        let Some(proposer) = self.validators.proposer() else {
            return;
        };
        let Some(pub_key) = proposer.pub_key else {
            return;
        };
        if proposal
            .verify_signature(&pub_key, self.chain_state.chain_id.as_str())
            .is_err()
        {
            return;
        }
        self.stash_current_parts();
        self.proposal_parts = Some(PartSet::from_header(
            proposal.block_id.part_set_header.clone(),
        ));
        self.apply_pending_parts(true);
        self.proposal = Some(proposal);
        if self.step <= Step::Propose && self.is_proposal_complete() {
            self.enter_prevote(self.height, self.round);
        }
    }

    pub fn on_part(&mut self, part: Part) {
        let before = self.height;
        if !self.add_proposal_part(part) {
            return;
        }
        self.note_complete_proposal();
        self.advance_after_commit(before);
    }

    /// `true` when the part set is complete. A part that arrives before the commit
    /// header is kept and retried when that header is installed. A bad proof
    /// against the commit header is dropped.
    fn add_proposal_part(&mut self, part: Part) -> bool {
        let Some(parts) = self.proposal_parts.as_mut() else {
            self.buffer_part(part);
            return false;
        };
        match parts.add_part(part.clone()) {
            Ok(_) => parts.is_complete(),
            Err(_) => {
                if self.step != Step::Commit {
                    self.buffer_part(part);
                }
                false
            }
        }
    }

    /// Keep parts from a part set that is about to be replaced.
    fn stash_current_parts(&mut self) {
        let Some(parts) = self.proposal_parts.take() else {
            return;
        };
        for index in 0..parts.total() {
            if let Some(part) = parts.get_part(index) {
                self.buffer_part(part.clone());
            }
        }
    }

    fn buffer_part(&mut self, part: Part) {
        // Keep earlier parts too. A bad proof can arrive after the real part and
        // must not be the only one left when the commit header shows up.
        let cap = self
            .proposal_parts
            .as_ref()
            .map(|parts| {
                usize::try_from(parts.total())
                    .unwrap_or(1)
                    .saturating_mul(2)
            })
            .unwrap_or(64)
            .max(8);
        if self.pending_parts.len() >= cap {
            return;
        }
        self.pending_parts.push(part);
    }

    fn apply_pending_parts(&mut self, keep_rejected: bool) {
        let pending = std::mem::take(&mut self.pending_parts);
        let mut rejected = Vec::new();
        for part in pending {
            let Some(parts) = self.proposal_parts.as_mut() else {
                rejected.push(part);
                continue;
            };
            if parts.add_part(part.clone()).is_err() {
                rejected.push(part);
            }
        }
        if keep_rejected {
            for part in rejected {
                self.buffer_part(part);
            }
        }
        if self.proposal_block.is_none() {
            self.decode_proposal_block();
        }
    }

    fn decode_proposal_block(&mut self) -> bool {
        let Some(parts) = self.proposal_parts.as_ref() else {
            return false;
        };
        if !parts.is_complete() {
            return false;
        }
        let mut bytes = Vec::new();
        for index in 0..parts.total() {
            let Some(part) = parts.get_part(index) else {
                return false;
            };
            bytes.extend_from_slice(&part.bytes);
        }
        let Ok(proto) = eld_tendermint_proto::types::Block::decode(bytes.as_slice()) else {
            return false;
        };
        let Ok(block) = Block::try_from_proto(&proto) else {
            return false;
        };
        self.proposal_block = Some(block);
        true
    }

    fn note_complete_proposal(&mut self) {
        if self.proposal_block.is_some() {
            if self.step == Step::Commit {
                self.try_finalize_commit();
            }
            return;
        }
        if !self.decode_proposal_block() {
            return;
        }
        if self.step <= Step::Propose && self.is_proposal_complete() {
            self.enter_prevote(self.height, self.round);
        }
        self.after_prevote(self.round);
        if self.step == Step::Commit {
            self.try_finalize_commit();
        }
    }

    pub fn on_vote(&mut self, vote: Vote) {
        if vote.height != self.height {
            return;
        }
        let added = self.votes.add_vote(&vote).unwrap_or(false);
        if !added {
            return;
        }
        match vote.vote_type {
            SignedMsgType::Prevote => self.after_prevote(vote.round),
            SignedMsgType::Precommit => self.after_precommit(vote.round),
            _ => {}
        }
    }

    /// Signs a nil precommit without adding it. Used to feed another validator.
    ///
    /// # Errors
    ///
    /// Returns the FilePV error.
    pub fn sign_nil_precommit(&mut self) -> Result<Vote, Error> {
        self.signed_vote(SignedMsgType::Precommit, BlockId::default())
    }

    pub(crate) fn on_timeout(&mut self, scheduled: &Scheduled) {
        if scheduled.height != self.height || scheduled.round != self.round {
            return;
        }
        match scheduled.step {
            Step::Propose if self.step == Step::Propose => {
                self.enter_prevote(self.height, self.round);
            }
            Step::PrevoteWait if self.step == Step::PrevoteWait => {
                self.enter_precommit(self.height, self.round);
            }
            Step::PrecommitWait if self.step == Step::PrecommitWait => {
                self.enter_new_round(self.height, self.round + 1);
            }
            Step::NewHeight if self.step == Step::NewHeight => {
                self.enter_new_round(self.height, 0);
            }
            _ => {}
        }
    }

    fn log_step(&self, message: &str) {
        let height = self.height.to_string();
        let round = self.round.to_string();
        log_line(
            Level::Info,
            "consensus",
            message,
            &[
                ("height", height.as_str()),
                ("round", round.as_str()),
                ("step", self.step.log_name()),
            ],
        );
    }

    fn enter_new_round(&mut self, height: i64, round: i32) {
        if self.height != height || round < self.round {
            return;
        }
        if round == self.round && self.step != Step::NewHeight {
            return;
        }
        if round > self.round {
            let times = round - self.round;
            if self.validators.increment_proposer_priority(times).is_err() {
                return;
            }
        }
        self.round = round;
        self.step = Step::NewRound;
        self.votes.set_round(round);
        self.log_step("enterNewRound");
        if round != 0 {
            self.proposal = None;
            self.proposal_block = None;
            self.proposal_parts = None;
        }
        self.enter_propose(height, round);
    }

    fn enter_propose(&mut self, height: i64, round: i32) {
        if self.height != height || round < self.round {
            return;
        }
        if round == self.round && self.step >= Step::Propose {
            return;
        }
        self.step = Step::Propose;
        let proposer = self
            .validators
            .proposer()
            .map(|validator| upper_hex(&validator.address))
            .unwrap_or_default();
        let height_field = self.height.to_string();
        let round_field = self.round.to_string();
        log_line(
            Level::Info,
            "consensus",
            "enterPropose",
            &[
                ("height", height_field.as_str()),
                ("round", round_field.as_str()),
                ("step", self.step.log_name()),
                ("proposer", proposer.as_str()),
            ],
        );
        self.schedule(self.config.propose(round), height, round, Step::Propose);
        if self.is_proposer() {
            self.decide_proposal(height, round);
        }
        if self.is_proposal_complete() {
            self.enter_prevote(height, round);
        }
    }

    fn decide_proposal(&mut self, height: i64, round: i32) {
        if self.replaying {
            return;
        }
        let (block, parts, pol_round) = if let (Some(block), Some(parts)) =
            (self.valid_block.clone(), self.valid_parts.clone())
        {
            (block, parts, self.valid_round)
        } else {
            let Some((block, parts)) = self.create_proposal_block(height) else {
                return;
            };
            (block, parts, -1)
        };
        let Some(hash) = block.hash() else {
            return;
        };
        let block_id = BlockId {
            hash: hash.as_bytes().to_vec(),
            part_set_header: parts.header(),
        };
        let mut proposal = Proposal::new(
            height,
            round,
            pol_round,
            block_id,
            self.chain_state.last_block_time,
        );
        if self
            .pv
            .sign_proposal(self.chain_state.chain_id.as_str(), &mut proposal)
            .is_err()
        {
            return;
        }
        self.proposal = Some(proposal.clone());
        self.proposal_block = Some(block);
        self.proposal_parts = Some(parts.clone());
        let now = Time::now();
        self.wal_sync(&wal::proposal_message(now, &proposal));
        for index in 0..parts.total() {
            if let Some(part) = parts.get_part(index) {
                self.wal_sync(&wal::part_message(now, height, round, part));
            }
        }
        self.outbox.push(Msg::Proposal(proposal));
        for index in 0..parts.total() {
            if let Some(part) = parts.get_part(index) {
                self.outbox.push(Msg::Part(part.clone()));
            }
        }
    }

    fn create_proposal_block(&mut self, height: i64) -> Option<(Block, PartSet)> {
        let max_bytes = self.chain_state.consensus_params.block.max_bytes;
        let max_gas = self.chain_state.consensus_params.block.max_gas;
        let txs = self
            .mempool
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .reap_max_bytes_max_gas(max_bytes, max_gas);
        self.reap_count += 1;
        let commit = if height == self.chain_state.initial_height {
            Some(Commit {
                height: 0,
                round: 0,
                block_id: BlockId::default(),
                signatures: Vec::new(),
            })
        } else {
            self.last_commit
                .as_ref()
                .and_then(|votes| votes.make_commit().ok())
        }?;
        let evidence = match &self.evidence {
            Some(pool) => {
                let max_bytes = self.chain_state.consensus_params.evidence.max_bytes;
                EvidenceList::new(pool.pending(max_bytes))
            }
            None => EvidenceList::new(Vec::new()),
        };
        let mut block = Block::make_block(height, txs, Some(commit), evidence);
        self.fill_header(&mut block);
        let parts = block.make_part_set(BLOCK_PART_SIZE_BYTES).ok()?;
        Some((block, parts))
    }

    fn fill_header(&self, block: &mut Block) {
        block.header.chain_id = self.chain_state.chain_id.clone();
        block.header.time = self.chain_state.last_block_time;
        block.header.last_block_id = self.chain_state.last_block_id.clone();
        if let Ok(hash) = self.validators.hash() {
            block.header.validators_hash = hash.as_bytes().to_vec();
        }
        if let Ok(hash) = self.chain_state.next_validators.hash() {
            block.header.next_validators_hash = hash.as_bytes().to_vec();
        }
        block.header.consensus_hash =
            eld_tendermint_types::hash_consensus_params(&self.chain_state.consensus_params)
                .as_bytes()
                .to_vec();
        block.header.app_hash = self.chain_state.app_hash.clone();
        block.header.last_results_hash = self.chain_state.last_results_hash.clone();
        block.header.proposer_address = self.address();
    }

    fn enter_prevote(&mut self, height: i64, round: i32) {
        if self.height != height || round < self.round {
            return;
        }
        if round == self.round && self.step >= Step::Prevote {
            return;
        }
        let block_id = if let Some(block) = &self.locked_block {
            block_id_of(block, self.locked_parts.as_ref())
        } else if self
            .proposal_parts
            .as_ref()
            .is_some_and(PartSet::is_complete)
        {
            self.proposal_block.as_ref().and_then(|block| {
                if self.block_ok(block) {
                    block_id_of(block, self.proposal_parts.as_ref())
                } else {
                    None
                }
            })
        } else {
            None
        };
        let _ = self.broadcast_vote(SignedMsgType::Prevote, block_id.unwrap_or_default());
        self.step = Step::Prevote;
        self.log_step("enterPrevote");
    }

    fn after_prevote(&mut self, round: i32) {
        if round != self.round || self.step < Step::Prevote {
            return;
        }
        let majority = self
            .votes
            .prevotes(round)
            .and_then(eld_tendermint_types::VoteSet::two_thirds_majority);
        let any = self
            .votes
            .prevotes(round)
            .is_some_and(eld_tendermint_types::VoteSet::has_two_thirds_any);
        if let Some(block_id) = majority {
            if !block_id.hash.is_empty()
                && self.valid_round < round
                && self
                    .proposal_block
                    .as_ref()
                    .and_then(Block::hash)
                    .is_some_and(|hash| hash.as_bytes() == block_id.hash.as_slice())
            {
                self.valid_round = round;
                self.valid_block = self.proposal_block.clone();
                self.valid_parts = self.proposal_parts.clone();
            }
            let nil = block_id.hash.is_empty();
            if nil || self.is_proposal_complete() {
                self.enter_precommit(self.height, round);
            }
        } else if any {
            self.enter_prevote_wait(self.height, round);
        }
    }

    fn enter_prevote_wait(&mut self, height: i64, round: i32) {
        if self.height != height || round != self.round || self.step >= Step::PrevoteWait {
            return;
        }
        self.step = Step::PrevoteWait;
        self.schedule(self.config.prevote(round), height, round, Step::PrevoteWait);
    }

    fn enter_precommit(&mut self, height: i64, round: i32) {
        if self.height != height || round < self.round {
            return;
        }
        if round == self.round && self.step >= Step::Precommit {
            return;
        }
        let majority = self
            .votes
            .prevotes(round)
            .and_then(eld_tendermint_types::VoteSet::two_thirds_majority);
        let Some(block_id) = majority else {
            let _ = self.broadcast_vote(SignedMsgType::Precommit, BlockId::default());
            self.step = Step::Precommit;
            self.log_step("enterPrecommit");
            return;
        };
        if block_id.hash.is_empty() {
            self.locked_block = None;
            self.locked_parts = None;
            let _ = self.broadcast_vote(SignedMsgType::Precommit, BlockId::default());
            self.step = Step::Precommit;
            self.log_step("enterPrecommit");
            return;
        }
        let proposal_matches = self
            .proposal_block
            .as_ref()
            .and_then(Block::hash)
            .is_some_and(|hash| hash.as_bytes() == block_id.hash.as_slice());
        if proposal_matches && self.block_ok(self.proposal_block.as_ref().expect("block")) {
            self.locked_block = self.proposal_block.clone();
            self.locked_parts = self.proposal_parts.clone();
            let _ = self.broadcast_vote(SignedMsgType::Precommit, block_id);
        } else if self
            .locked_block
            .as_ref()
            .and_then(Block::hash)
            .is_some_and(|hash| hash.as_bytes() == block_id.hash.as_slice())
        {
            let _ = self.broadcast_vote(SignedMsgType::Precommit, block_id);
        } else {
            self.locked_block = None;
            self.locked_parts = None;
            let _ = self.broadcast_vote(SignedMsgType::Precommit, BlockId::default());
        }
        self.step = Step::Precommit;
        self.log_step("enterPrecommit");
    }

    fn after_precommit(&mut self, round: i32) {
        let majority = self
            .votes
            .precommits(round)
            .and_then(eld_tendermint_types::VoteSet::two_thirds_majority);
        let any = self
            .votes
            .precommits(round)
            .is_some_and(eld_tendermint_types::VoteSet::has_two_thirds_any);
        if let Some(block_id) = majority {
            if block_id.hash.is_empty() {
                self.enter_precommit_wait(self.height, round);
            } else {
                let before = self.height;
                self.enter_commit(self.height, round);
                self.advance_after_commit(before);
            }
        } else if self.round <= round && any {
            self.enter_precommit_wait(self.height, round);
        }
    }

    fn enter_precommit_wait(&mut self, height: i64, round: i32) {
        if self.height != height || round != self.round || self.step >= Step::PrecommitWait {
            return;
        }
        self.step = Step::PrecommitWait;
        self.schedule(
            self.config.precommit(round),
            height,
            round,
            Step::PrecommitWait,
        );
    }

    /// Start the next height only after this one has been saved.
    ///
    /// The round starts on the following tick, so one reactor poll commits one
    /// block when the local validators already have a quorum.
    fn advance_after_commit(&mut self, before: i64) {
        if self.height != before && self.config.skip_timeout_commit {
            self.schedule(
                eld_tendermint_config::Duration::from_nanos(0),
                self.height,
                0,
                Step::NewHeight,
            );
        }
    }

    fn enter_commit(&mut self, height: i64, commit_round: i32) {
        if self.height != height || self.step >= Step::Commit {
            return;
        }
        let Some(block_id) = self
            .votes
            .precommits(commit_round)
            .and_then(eld_tendermint_types::VoteSet::two_thirds_majority)
        else {
            return;
        };
        if block_id.hash.is_empty() {
            return;
        }
        // A PrecommitWait timeout must not start another round and drop this header.
        self.step = Step::Commit;
        self.commit_round = Some(commit_round);
        self.install_commit_header(&block_id);
        self.try_finalize_commit();
    }

    fn install_commit_header(&mut self, block_id: &BlockId) {
        let block_matches = self
            .proposal_block
            .as_ref()
            .and_then(Block::hash)
            .is_some_and(|hash| hash.as_bytes() == block_id.hash.as_slice());
        let header_matches = self.proposal_parts.as_ref().is_some_and(|parts| {
            let header = parts.header();
            header.total == block_id.part_set_header.total
                && header.hash == block_id.part_set_header.hash
        });
        if !(block_matches && header_matches) {
            if !header_matches {
                self.stash_current_parts();
                self.proposal_block = None;
                self.proposal_parts = Some(PartSet::from_header(block_id.part_set_header.clone()));
            } else {
                self.proposal_block = None;
            }
        }
        self.apply_pending_parts(false);
    }

    fn try_finalize_commit(&mut self) {
        if self.step != Step::Commit {
            return;
        }
        let Some(commit_round) = self.commit_round else {
            return;
        };
        let Some(block_id) = self
            .votes
            .precommits(commit_round)
            .and_then(eld_tendermint_types::VoteSet::two_thirds_majority)
        else {
            return;
        };
        if block_id.hash.is_empty() {
            return;
        }
        let Some(block) = self.proposal_block.clone() else {
            return;
        };
        let Some(block_hash) = block.hash() else {
            return;
        };
        if block_hash.as_bytes() != block_id.hash.as_slice() {
            return;
        }
        let Some(parts) = self.proposal_parts.clone() else {
            return;
        };
        let id = BlockId {
            hash: block_hash.as_bytes().to_vec(),
            part_set_header: parts.header(),
        };
        let hash = upper_hex(block_hash.as_bytes());
        let height = self.height.to_string();
        let round = commit_round.to_string();
        log_line(
            Level::Info,
            "consensus",
            "Finalizing commit",
            &[
                ("height", height.as_str()),
                ("round", round.as_str()),
                ("hash", hash.as_str()),
            ],
        );
        let applied = match apply_block(&self.chain_state, &id, &block, &mut self.exec) {
            Ok(applied) => applied,
            Err(err) => {
                let err = err.to_string();
                log_line(
                    Level::Error,
                    "consensus",
                    "failed to apply block",
                    &[("err", err.as_str())],
                );
                return;
            }
        };
        let seen = self
            .votes
            .precommits(commit_round)
            .and_then(|votes| votes.make_commit().ok());
        let Some(seen) = seen else {
            return;
        };
        if self.block_store.save_block(&block, &parts, &seen).is_err() {
            return;
        }
        let retain_height = applied.retain_height;
        if retain_height > self.block_store.base() {
            let _ = self.block_store.prune_blocks(retain_height);
        }
        if let Some(index) = &self.tx_index {
            if index.index_committed(&block, &applied.deliver_txs).is_err() {
                return;
            }
        }
        if let Some(events) = &self.events {
            events.on_commit(&block, &seen, &applied.deliver_txs);
        }
        // Drop committed txs before the next height reaps, or the same hash is indexed again.
        let _ = self
            .mempool
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .update(
                block.header.height,
                block.data.as_slice(),
                &applied.deliver_txs,
                None,
                None,
            );
        if let Some(pool) = &self.evidence {
            pool.mark_committed(&block.evidence);
            pool.update_state(&applied.state);
        }
        let committed = self.height;
        let committed_height = committed.to_string();
        let committed_hash = upper_hex(block_hash.as_bytes());
        let app_hash = upper_hex(&applied.state.app_hash);
        let num_txs = block.data.as_slice().len().to_string();
        self.commit_round = None;
        self.pending_parts.clear();
        self.wal_sync(&wal::end_height_message(Time::now(), committed));
        self.last_commit = self.votes.precommits(commit_round).cloned();
        self.chain_state = applied.state;
        self.validators = self.chain_state.validators.copy();
        self.height = self.chain_state.last_block_height + 1;
        self.round = 0;
        self.step = Step::NewHeight;
        self.proposal = None;
        self.proposal_block = None;
        self.proposal_parts = None;
        self.locked_block = None;
        self.locked_parts = None;
        self.valid_block = None;
        self.valid_parts = None;
        self.valid_round = -1;
        self.timeout = None;
        self.votes = HeightVoteSet::new(
            self.chain_state.chain_id.as_str(),
            self.height,
            self.validators.copy(),
        );
        log_line(
            Level::Info,
            "consensus",
            "Committed block",
            &[
                ("height", committed_height.as_str()),
                ("hash", committed_hash.as_str()),
                ("app_hash", app_hash.as_str()),
                ("num_txs", num_txs.as_str()),
            ],
        );
    }

    fn block_ok(&self, block: &Block) -> bool {
        eld_tendermint_state::validate_block(&self.chain_state, block).is_ok()
    }

    fn is_proposal_complete(&self) -> bool {
        if self.proposal.is_none() || self.proposal_block.is_none() {
            return false;
        }
        let pol_round = self
            .proposal
            .as_ref()
            .map(|proposal| proposal.pol_round)
            .unwrap_or(-1);
        if pol_round < 0 {
            return true;
        }
        self.votes
            .prevotes(pol_round)
            .is_some_and(eld_tendermint_types::VoteSet::has_two_thirds_majority)
    }

    fn is_proposer(&self) -> bool {
        self.validators
            .proposer()
            .is_some_and(|proposer| proposer.address == self.address())
    }

    fn broadcast_vote(&mut self, vote_type: SignedMsgType, block_id: BlockId) -> Result<(), Error> {
        if self.replaying {
            return Ok(());
        }
        let vote = self.signed_vote(vote_type, block_id)?;
        self.wal_sync(&wal::vote_message(Time::now(), &vote));
        self.outbox.push(Msg::Vote(vote));
        Ok(())
    }

    fn signed_vote(&mut self, vote_type: SignedMsgType, block_id: BlockId) -> Result<Vote, Error> {
        let address = self.address();
        let index = self
            .validators
            .validators()
            .iter()
            .position(|validator| validator.address == address)
            .ok_or(Error::NotValidator)?;
        let mut vote = Vote {
            vote_type,
            height: self.height,
            round: self.round,
            block_id,
            timestamp: self.chain_state.last_block_time,
            validator_address: address,
            validator_index: i32::try_from(index).unwrap_or(i32::MAX),
            signature: Vec::new(),
        };
        self.pv
            .sign_vote(self.chain_state.chain_id.as_str(), &mut vote)
            .map_err(Error::Privval)?;
        Ok(vote)
    }

    fn schedule(
        &mut self,
        delay: eld_tendermint_config::Duration,
        height: i64,
        round: i32,
        step: Step,
    ) {
        self.timeout = Some(Scheduled {
            delay_nanos: delay.as_nanos(),
            height,
            round,
            step,
        });
        if self.replaying {
            return;
        }
        self.wal_sync(&wal::timeout_message(
            Time::now(),
            delay.as_nanos(),
            height,
            round,
            step.as_wal(),
        ));
    }

    /// `catchupReplay`. An `EndHeight` for this height means the block is stored,
    /// so the next height starts at round 0 and this height is not replayed.
    fn catchup(&mut self) -> Result<(), Error> {
        while self
            .wal
            .as_ref()
            .expect("wal")
            .messages_after_end_height(self.height)?
            .is_some()
        {
            self.height += 1;
            self.round = 0;
            self.step = Step::NewHeight;
            self.votes = HeightVoteSet::new(
                self.chain_state.chain_id.as_str(),
                self.height,
                self.validators.copy(),
            );
        }
        let end_height = if self.height == self.chain_state.initial_height {
            0
        } else {
            self.height - 1
        };
        let Some(messages) = self
            .wal
            .as_ref()
            .expect("wal")
            .messages_after_end_height(end_height)?
        else {
            return Err(Error::MissingEndHeight { height: end_height });
        };
        self.replaying = true;
        for msg in messages {
            self.read_replay(&msg)?;
        }
        self.replaying = false;
        Ok(())
    }

    fn read_replay(
        &mut self,
        msg: &eld_tendermint_proto::consensus::TimedWalMessage,
    ) -> Result<(), Error> {
        match wal::replay_of(msg)? {
            Replay::Ignored | Replay::EndHeight(_) => Ok(()),
            Replay::Proposal(proposal) => {
                self.on_proposal(proposal);
                Ok(())
            }
            Replay::Part { part, .. } => {
                self.on_part(part);
                Ok(())
            }
            Replay::Vote(vote) => self.replay_vote(vote),
            Replay::Timeout {
                height,
                round,
                step,
                ..
            } => {
                let Some(step) = Step::from_wal(step) else {
                    return Ok(());
                };
                let scheduled = Scheduled {
                    delay_nanos: 0,
                    height,
                    round,
                    step,
                };
                self.on_timeout(&scheduled);
                Ok(())
            }
        }
    }

    /// Deliver a WAL vote. Our own vote is not signed again. A different block id
    /// at the same height, round, and step is `FilePV`'s conflicting-data error.
    fn replay_vote(&mut self, vote: Vote) -> Result<(), Error> {
        if vote.validator_address == self.address() {
            self.reject_conflicting_vote(&vote)?;
        }
        self.on_vote(vote);
        Ok(())
    }

    fn reject_conflicting_vote(&mut self, vote: &Vote) -> Result<(), Error> {
        let step = match vote.vote_type {
            SignedMsgType::Prevote => STEP_PREVOTE,
            SignedMsgType::Precommit => STEP_PRECOMMIT,
            _ => return Ok(()),
        };
        let stored = {
            let state = self.pv.last_sign_state();
            if state.height != vote.height || state.round != vote.round || state.step != step {
                return Ok(());
            }
            state.sign_bytes.clone()
        };
        let chain_id = self.chain_state.chain_id.as_str().to_owned();
        let sign_bytes = vote.sign_bytes(&chain_id);
        if stored.as_deref() == Some(sign_bytes.as_slice()) {
            return Ok(());
        }
        let mut copy = vote.clone();
        self.pv
            .sign_vote(&chain_id, &mut copy)
            .map_err(Error::Privval)
    }

    fn wal_sync(&mut self, msg: &eld_tendermint_proto::consensus::TimedWalMessage) {
        if self.replaying {
            return;
        }
        let Some(wal) = self.wal.as_mut() else {
            return;
        };
        if let Err(err) = wal.write_message(msg) {
            panic!("failed to write consensus WAL: {err}");
        }
    }
}

fn block_id_of(block: &Block, parts: Option<&PartSet>) -> Option<BlockId> {
    let hash = block.hash()?;
    let parts = parts?;
    Some(BlockId {
        hash: hash.as_bytes().to_vec(),
        part_set_header: parts.header(),
    })
}
