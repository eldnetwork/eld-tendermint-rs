//! `HeightVoteSet` without peer catchup rounds.

use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{Error, ValidatorSet, Vote, VoteSet};

struct RoundVotes {
    prevotes: VoteSet,
    precommits: VoteSet,
}

/// Prevotes and precommits for rounds `0..=round` at one height.
pub struct HeightVoteSet {
    chain_id: String,
    height: i64,
    validators: ValidatorSet,
    round: i32,
    rounds: Vec<RoundVotes>,
}

impl HeightVoteSet {
    #[must_use]
    pub fn new(chain_id: impl Into<String>, height: i64, validators: ValidatorSet) -> Self {
        let chain_id = chain_id.into();
        let round0 = RoundVotes {
            prevotes: VoteSet::new(
                chain_id.clone(),
                height,
                0,
                SignedMsgType::Prevote,
                validators.copy(),
            ),
            precommits: VoteSet::new(
                chain_id.clone(),
                height,
                0,
                SignedMsgType::Precommit,
                validators.copy(),
            ),
        };
        Self {
            chain_id,
            height,
            validators,
            round: 0,
            rounds: vec![round0],
        }
    }

    /// Adds any missing round in `0..=round`.
    pub fn set_round(&mut self, round: i32) {
        if round < 0 {
            return;
        }
        let target = usize::try_from(round).unwrap_or(0);
        while self.rounds.len() <= target {
            let next = i32::try_from(self.rounds.len()).unwrap_or(i32::MAX);
            self.rounds.push(RoundVotes {
                prevotes: VoteSet::new(
                    self.chain_id.clone(),
                    self.height,
                    next,
                    SignedMsgType::Prevote,
                    self.validators.copy(),
                ),
                precommits: VoteSet::new(
                    self.chain_id.clone(),
                    self.height,
                    next,
                    SignedMsgType::Precommit,
                    self.validators.copy(),
                ),
            });
        }
        if round > self.round {
            self.round = round;
        }
    }

    pub fn prevotes(&self, round: i32) -> Option<&VoteSet> {
        self.rounds
            .get(usize::try_from(round).ok()?)
            .map(|round| &round.prevotes)
    }

    pub fn precommits(&self, round: i32) -> Option<&VoteSet> {
        self.rounds
            .get(usize::try_from(round).ok()?)
            .map(|round| &round.precommits)
    }

    /// `HeightVoteSet.SetPeerMaj23`. A round this set does not have yet is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`eld_tendermint_types::Error::UnexpectedVoteStep`] for a vote type that
    /// is not a prevote or precommit, and [`eld_tendermint_types::Error::ConflictingPeerMaj23`]
    /// when `peer_id` already claimed another block id for this round and type.
    pub fn set_peer_maj23(
        &mut self,
        round: i32,
        vote_type: SignedMsgType,
        peer_id: &str,
        block_id: &eld_tendermint_types::BlockId,
    ) -> Result<(), Error> {
        if vote_type != SignedMsgType::Prevote && vote_type != SignedMsgType::Precommit {
            return Err(Error::UnexpectedVoteStep);
        }
        let Some(round_index) = usize::try_from(round).ok() else {
            return Ok(());
        };
        let Some(round_votes) = self.rounds.get_mut(round_index) else {
            return Ok(());
        };
        let votes = match vote_type {
            SignedMsgType::Prevote => &mut round_votes.prevotes,
            SignedMsgType::Precommit => &mut round_votes.precommits,
            _ => return Err(Error::UnexpectedVoteStep),
        };
        votes.set_peer_maj23(peer_id, block_id)
    }

    /// # Errors
    ///
    /// Returns the [`VoteSet::add_vote`] error.
    pub fn add_vote(&mut self, vote: &Vote) -> Result<bool, Error> {
        if vote.round < 0 {
            return Err(Error::UnexpectedVoteStep);
        }
        self.set_round(vote.round);
        let round = usize::try_from(vote.round).map_err(|_| Error::UnexpectedVoteStep)?;
        match vote.vote_type {
            SignedMsgType::Prevote => self.rounds[round].prevotes.add_vote(vote),
            SignedMsgType::Precommit => self.rounds[round].precommits.add_vote(vote),
            _ => Err(Error::UnexpectedVoteStep),
        }
    }
}
