//! `types.VoteSet`. Collects prevotes or precommits for one height and round.
//!
//! A block id, including a nil block id, has a majority when its voting power is greater
//! than `total * 2 / 3`. Conflicting votes from one validator are not added. There is no
//! peer-maj23 tracking.

use std::collections::HashMap;

use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};

use crate::{BlockId, Commit, CommitSig, Error, ValidatorSet, Vote};

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
}

#[derive(Clone, Debug)]
struct BlockVotes {
    sum: i64,
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
