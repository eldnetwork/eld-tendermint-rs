//! One validator's round state. No network.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use eld_tendermint_config::ConsensusConfig;
use eld_tendermint_evidence::ProposalEvidence;
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::{FilePV, STEP_PRECOMMIT, STEP_PREVOTE};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::{
    App as ExecApp, CommitEvents, IndexTxs, State as ChainState, apply_block,
};
use eld_tendermint_store::{BlockStore, Db, MemDb};
use eld_tendermint_types::{
    BLOCK_PART_SIZE_BYTES, Block, BlockId, Commit, EvidenceList, Part, PartSet, Proposal, Time,
    ValidatorSet, Vote,
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
    pv: FilePV,
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
}

impl<E: ExecApp, C: MempoolApp> Node<E, C, MemDb> {
    /// `NewNode` with an in-memory block store.
    ///
    /// # Errors
    ///
    /// Returns a mempool or validator-set error.
    pub fn start(
        config: ConsensusConfig,
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
        pv: FilePV,
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
            pv,
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
        self.pv.last_sign_state.signature.clone()
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
        self.proposal_parts = Some(PartSet::from_header(
            proposal.block_id.part_set_header.clone(),
        ));
        self.proposal = Some(proposal);
        if self.step <= Step::Propose && self.is_proposal_complete() {
            self.enter_prevote(self.height, self.round);
        }
    }

    pub fn on_part(&mut self, part: Part) {
        let complete = {
            let Some(parts) = self.proposal_parts.as_mut() else {
                return;
            };
            let added = parts.add_part(part).ok() == Some(true);
            if !added && !parts.is_complete() {
                return;
            }
            parts.is_complete()
        };
        if !complete {
            return;
        }
        if self.proposal_block.is_some() {
            return;
        }
        let Some(parts) = self.proposal_parts.as_ref() else {
            return;
        };
        let mut bytes = Vec::new();
        for index in 0..parts.total() {
            let Some(part) = parts.get_part(index) else {
                return;
            };
            bytes.extend_from_slice(&part.bytes);
        }
        let Ok(proto) = eld_tendermint_proto::types::Block::decode(bytes.as_slice()) else {
            return;
        };
        let Ok(block) = Block::try_from_proto(&proto) else {
            return;
        };
        self.proposal_block = Some(block);
        if self.step <= Step::Propose && self.is_proposal_complete() {
            self.enter_prevote(self.height, self.round);
        }
        self.after_prevote(self.round);
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
            _ => {}
        }
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
            return;
        };
        if block_id.hash.is_empty() {
            self.locked_block = None;
            self.locked_parts = None;
            let _ = self.broadcast_vote(SignedMsgType::Precommit, BlockId::default());
            self.step = Step::Precommit;
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
        let all = self
            .votes
            .precommits(round)
            .is_some_and(eld_tendermint_types::VoteSet::has_all);
        if let Some(block_id) = majority {
            if block_id.hash.is_empty() {
                self.enter_precommit_wait(self.height, round);
            } else {
                self.enter_commit(self.height, round);
                if self.config.skip_timeout_commit && all {
                    self.enter_new_round(self.height, 0);
                }
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

    fn enter_commit(&mut self, height: i64, commit_round: i32) {
        if self.height != height || self.step >= Step::Commit {
            return;
        }
        self.step = Step::Commit;
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
        let Ok(applied) = apply_block(&self.chain_state, &id, &block, &mut self.exec) else {
            return;
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
            let state = &self.pv.last_sign_state;
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
