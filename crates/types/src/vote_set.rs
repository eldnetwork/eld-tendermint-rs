//! `types.VoteSet`. Collects prevotes or precommits for one height and round.
//!
//! A block id, including a nil block id, has a majority when its voting power is greater
//! than `total * 2 / 3`. Conflicting votes from one validator are not added to the
//! canonical list. `set_peer_maj23` keeps a conflicting vote on the block a peer claimed.

use std::collections::HashMap;

use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};

use crate::{BitArray, BlockId, Commit, CommitSig, Error, ValidatorSet, Vote};

/// Votes for one height, round, and signed-message type.
#[derive(Clone, Debug)]
pub struct VoteSet {
    chain_id: String,
    height: i64,
    round: i32,
    vote_type: SignedMsgType,
    validators: ValidatorSet,
    votes: Vec<Option<Vote>>,
    /// Voting power of validators whose first vote was accepted.
    sum: i64,
    maj23: Option<BlockId>,
    by_block: HashMap<Vec<u8>, BlockVotes>,
    peer_maj23s: HashMap<String, BlockId>,
}

#[derive(Clone, Debug)]
struct BlockVotes {
    sum: i64,
    /// A peer claimed +2/3 for this block, so conflicting votes may be kept here.
    peer_maj23: bool,
    votes: Vec<Option<Vote>>,
}

impl VoteSet {
    #[must_use]
    pub fn new(
        chain_id: impl Into<String>,
        height: i64,
        round: i32,
        vote_type: SignedMsgType,
        validators: ValidatorSet,
    ) -> Self {
        let n = validators.validators().len();
        Self {
            chain_id: chain_id.into(),
            height,
            round,
            vote_type,
            validators,
            votes: vec![None; n],
            sum: 0,
            maj23: None,
            by_block: HashMap::new(),
            peer_maj23s: HashMap::new(),
        }
    }

    #[must_use]
    pub fn has_two_thirds_majority(&self) -> bool {
        self.maj23.is_some()
    }

    /// Seen power, excluding conflicts, is greater than `total * 2 / 3`.
    #[must_use]
    pub fn has_two_thirds_any(&self) -> bool {
        self.sum > self.validators.total_voting_power() * 2 / 3
    }

    #[must_use]
    pub fn has_all(&self) -> bool {
        self.sum == self.validators.total_voting_power()
    }

    #[must_use]
    pub fn two_thirds_majority(&self) -> Option<BlockId> {
        self.maj23.clone()
    }

    #[must_use]
    pub const fn height(&self) -> i64 {
        self.height
    }

    #[must_use]
    pub const fn round(&self) -> i32 {
        self.round
    }

    /// `VoteSet.Size`. One slot per validator.
    #[must_use]
    pub fn validator_size(&self) -> usize {
        self.votes.len()
    }

    /// `VoteSet.IsCommit`. Precommits that have reached a +2/3 block id.
    #[must_use]
    pub fn is_commit(&self) -> bool {
        self.vote_type == SignedMsgType::Precommit && self.maj23.is_some()
    }

    /// Number of validator slots that have a canonical vote.
    #[must_use]
    pub fn vote_count(&self) -> usize {
        self.votes.iter().filter(|vote| vote.is_some()).count()
    }

    /// `AddVote`. `Ok(false)` is a duplicate. A conflicting block id is [`Error::ConflictingVote`].
    ///
    /// # Errors
    ///
    /// Returns a step, index, signature, or conflict error.
    pub fn add_vote(&mut self, vote: &Vote) -> Result<bool, Error> {
        if vote.height != self.height
            || vote.round != self.round
            || vote.vote_type != self.vote_type
        {
            return Err(Error::UnexpectedVoteStep);
        }
        let index = usize::try_from(vote.validator_index).map_err(|_| Error::InvalidVoteIndex)?;
        let validator = self
            .validators
            .validators()
            .get(index)
            .ok_or(Error::InvalidVoteIndex)?;
        if validator.address != vote.validator_address {
            return Err(Error::InvalidVoteIndex);
        }
        let pub_key = validator.pub_key.as_ref().ok_or(Error::MissingPubKey)?;
        vote.verify_signature(pub_key, &self.chain_id)?;
        let block_key = vote.block_id.key();
        if let Some(existing) = &self.votes[index] {
            if existing.block_id == vote.block_id {
                if existing.signature == vote.signature {
                    return Ok(false);
                }
                return Err(Error::DuplicateVoteSignature);
            }
            let block_key = vote.block_id.key();
            if let Some(slot) = self.by_block.get_mut(&block_key) {
                if slot.peer_maj23 && slot.votes[index].is_none() {
                    slot.sum += validator.voting_power;
                    slot.votes[index] = Some(vote.clone());
                }
            }
            return Err(Error::ConflictingVote);
        }
        let power = validator.voting_power;
        let n = self.votes.len();
        let quorum = self.validators.total_voting_power() * 2 / 3 + 1;
        let already_maj = self.maj23.is_some();
        self.votes[index] = Some(vote.clone());
        self.sum += power;
        let block_votes = {
            let slot = self
                .by_block
                .entry(block_key)
                .or_insert_with(|| BlockVotes {
                    sum: 0,
                    peer_maj23: false,
                    votes: vec![None; n],
                });
            let orig = slot.sum;
            slot.sum += power;
            slot.votes[index] = Some(vote.clone());
            if !already_maj && orig < quorum && slot.sum >= quorum {
                Some(slot.votes.clone())
            } else {
                None
            }
        };
        if let Some(block_votes) = block_votes {
            self.maj23 = Some(vote.block_id.clone());
            for (i, block_vote) in block_votes.into_iter().enumerate() {
                if block_vote.is_some() {
                    self.votes[i] = block_vote;
                }
            }
        }
        Ok(true)
    }

    /// `VoteSet.BitArray`. Which validator indexes have a canonical vote.
    #[must_use]
    pub fn bit_array(&self) -> Option<BitArray> {
        let mut bits = BitArray::new(i64::try_from(self.votes.len()).unwrap_or(0))?;
        for (index, vote) in self.votes.iter().enumerate() {
            if vote.is_some() {
                bits.set_index(i64::try_from(index).unwrap_or(0), true);
            }
        }
        Some(bits)
    }

    /// `VoteSet.GetByIndex`.
    #[must_use]
    pub fn get_by_index(&self, index: i32) -> Option<&Vote> {
        self.votes.get(usize::try_from(index).ok()?)?.as_ref()
    }

    /// `VoteSet.BitArrayByBlockID`. `None` when this set has no votes for `block_id`.
    #[must_use]
    pub fn bit_array_by_block_id(&self, block_id: &BlockId) -> Option<BitArray> {
        let votes = self.by_block.get(&block_id.key())?;
        let mut bits = BitArray::new(i64::try_from(votes.votes.len()).unwrap_or(0))?;
        for (index, vote) in votes.votes.iter().enumerate() {
            if vote.is_some() {
                bits.set_index(i64::try_from(index).unwrap_or(0), true);
            }
        }
        Some(bits)
    }

    /// `VoteSet.SetPeerMaj23`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConflictingPeerMaj23`] when `peer_id` already claimed another block id.
    pub fn set_peer_maj23(&mut self, peer_id: &str, block_id: &BlockId) -> Result<(), Error> {
        if let Some(existing) = self.peer_maj23s.get(peer_id) {
            if existing == block_id {
                return Ok(());
            }
            return Err(Error::ConflictingPeerMaj23);
        }
        self.peer_maj23s
            .insert(peer_id.to_owned(), block_id.clone());
        let key = block_id.key();
        if let Some(votes) = self.by_block.get_mut(&key) {
            votes.peer_maj23 = true;
            return Ok(());
        }
        let n = self.votes.len();
        self.by_block.insert(
            key,
            BlockVotes {
                sum: 0,
                peer_maj23: true,
                votes: vec![None; n],
            },
        );
        Ok(())
    }

    /// `MakeCommit`. Only precommits, and only once a block id has +2/3.
    ///
    /// A vote for a different block id becomes an absent signature. Nil votes stay nil.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedVoteStep`] when this set is not precommits, or
    /// [`Error::NoCommitSignatures`] when there is no majority.
    pub fn make_commit(&self) -> Result<Commit, Error> {
        if self.vote_type != SignedMsgType::Precommit {
            return Err(Error::UnexpectedVoteStep);
        }
        let maj23 = self.maj23.clone().ok_or(Error::NoCommitSignatures)?;
        let mut signatures = Vec::with_capacity(self.votes.len());
        for vote in &self.votes {
            let Some(vote) = vote else {
                signatures.push(CommitSig::absent());
                continue;
            };
            if !vote.block_id.hash.is_empty() && vote.block_id != maj23 {
                signatures.push(CommitSig::absent());
                continue;
            }
            let flag = if vote.block_id.hash.is_empty() {
                BlockIdFlag::Nil
            } else {
                BlockIdFlag::Commit
            };
            signatures.push(CommitSig {
                block_id_flag: flag,
                validator_address: vote.validator_address.clone(),
                timestamp: vote.timestamp,
                signature: vote.signature.clone(),
            });
        }
        Ok(Commit {
            height: self.height,
            round: self.round,
            block_id: maj23,
            signatures,
        })
    }
}
