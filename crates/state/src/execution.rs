//! `BlockExecutor.ApplyBlock` without mempool, evidence, or events.
//!
//! `BlockExecutor` lives in `state/execution.go`.

use eld_tendermint_crypto::{hash_from_byte_slices, pub_key_from_proto};
use eld_tendermint_proto::abci::{
    LastCommitInfo, RequestBeginBlock, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock,
    ResponseCommit, ResponseDeliverTx, ResponseEndBlock, ValidatorUpdate, VoteInfo,
};
use eld_tendermint_types::{
    ABCI_PUBKEY_TYPE_ED25519, Block, BlockId, Validator, hash_consensus_params, median_time,
};
use prost::Message;
use prost::bytes::Bytes;

use crate::error::Error;
use crate::state::State;

/// The four ABCI calls `ApplyBlock` makes. An in-process app implements this.
/// A socket client is not required.
pub trait App {
    /// # Errors
    ///
    /// Returns an application error. `ApplyBlock` does not call this until the
    /// block has passed validation.
    fn begin_block(&mut self, request: RequestBeginBlock) -> Result<ResponseBeginBlock, Error>;

    /// # Errors
    ///
    /// Returns an application error.
    fn deliver_tx(&mut self, request: RequestDeliverTx) -> Result<ResponseDeliverTx, Error>;

    /// # Errors
    ///
    /// Returns an application error.
    fn end_block(&mut self, request: RequestEndBlock) -> Result<ResponseEndBlock, Error>;

    /// # Errors
    ///
    /// Returns an application error. `ResponseCommit.data` becomes the next `app_hash`.
    fn commit(&mut self) -> Result<ResponseCommit, Error>;
}

/// `ApplyBlock`.
///
/// Validates the block, runs BeginBlock, DeliverTx, EndBlock, then Commit.
/// Validator updates from EndBlock are applied to `next_validators` and become
/// the current set only when the following block is applied.
///
/// `block_id` is stored as `last_block_id`. A height-1 block may have no last
/// commit, and `Block::hash` is `None` in that case, so the caller passes the id.
///
/// # Errors
///
/// Returns a validation error before any ABCI call, or an update or app error after.
/// `deliver_txs` is in block order so the tx index can store each result.
pub fn apply_block(
    state: &State,
    block_id: &BlockId,
    block: &Block,
    app: &mut impl App,
) -> Result<AppliedBlock, Error> {
    validate_block(state, block)?;

    let round = block
        .last_commit
        .as_ref()
        .map(|commit| commit.round)
        .unwrap_or(0);
    let votes = if block.header.height == state.initial_height {
        Vec::new()
    } else {
        last_commit_votes(state, block)
    };
    let hash = block
        .hash()
        .map(|hash| Bytes::copy_from_slice(hash.as_bytes()))
        .unwrap_or_default();
    app.begin_block(RequestBeginBlock {
        hash,
        header: Some(block.header.to_proto()),
        last_commit_info: Some(LastCommitInfo { round, votes }),
        byzantine_validators: Vec::new(),
    })?;

    let mut deliver_txs = Vec::with_capacity(block.data.as_slice().len());
    for tx in block.data.as_slice() {
        deliver_txs.push(app.deliver_tx(RequestDeliverTx {
            tx: Bytes::copy_from_slice(tx.as_bytes()),
        })?);
    }
    let end = app.end_block(RequestEndBlock {
        height: block.header.height,
    })?;

    let updates = validator_updates(&end.validator_updates, &state.consensus_params)?;
    let mut last_height_validators_changed = state.last_height_validators_changed;
    let mut next_validators = state.next_validators.copy();
    if !updates.is_empty() {
        next_validators.update_with_change_set(&updates)?;
        last_height_validators_changed = block.header.height + 2;
    }
    next_validators.increment_proposer_priority(1)?;

    let mut consensus_params = state.consensus_params.clone();
    let mut last_height_consensus_params_changed = state.last_height_consensus_params_changed;
    let mut version = state.version.clone();
    if let Some(update) = &end.consensus_param_updates {
        consensus_params = consensus_params.update(update);
        consensus_params.validate()?;
        version.consensus.app = consensus_params.version.app_version;
        last_height_consensus_params_changed = block.header.height + 1;
    }

    let commit = app.commit()?;
    let retain_height = commit.retain_height;
    Ok(AppliedBlock {
        retain_height,
        state: State {
            version,
            chain_id: state.chain_id.clone(),
            initial_height: state.initial_height,
            last_block_height: block.header.height,
            last_block_id: block_id.clone(),
            last_block_time: block.header.time,
            next_validators,
            validators: state.next_validators.copy(),
            last_validators: state.validators.copy(),
            last_height_validators_changed,
            consensus_params,
            last_height_consensus_params_changed,
            last_results_hash: results_hash(&deliver_txs),
            app_hash: commit.data.to_vec(),
        },
        deliver_txs,
    })
}

/// State after `ApplyBlock`, plus each `ResponseDeliverTx` in block order.
pub struct AppliedBlock {
    pub state: State,
    pub deliver_txs: Vec<ResponseDeliverTx>,
    /// `ResponseCommit.retain_height`. Zero means the app did not ask for pruning.
    pub retain_height: i64,
}

impl std::fmt::Debug for AppliedBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppliedBlock")
            .field("state", &self.state)
            .field("deliver_txs", &self.deliver_txs.len())
            .field("retain_height", &self.retain_height)
            .finish()
    }
}

/// Height, chain id, and header-hash checks from `ApplyBlock`, before any ABCI call.
///
/// # Errors
///
/// Returns the first mismatched field.
pub fn validate_block(state: &State, block: &Block) -> Result<(), Error> {
    let wanted = if state.last_block_height == 0 {
        state.initial_height
    } else {
        state.last_block_height + 1
    };
    if block.header.height != wanted {
        return Err(Error::WrongHeight {
            wanted,
            got: block.header.height,
        });
    }
    if block.header.chain_id != state.chain_id {
        return Err(Error::WrongChainId {
            wanted: state.chain_id.as_str().to_owned(),
            got: block.header.chain_id.as_str().to_owned(),
        });
    }
    if block.header.last_block_id != state.last_block_id {
        return Err(Error::WrongLastBlockId);
    }
    if block.header.app_hash != state.app_hash {
        return Err(Error::WrongAppHash);
    }
    if block.header.last_results_hash != state.last_results_hash {
        return Err(Error::WrongLastResultsHash);
    }
    let validators_hash = state.validators.hash()?;
    if block.header.validators_hash.as_slice() != validators_hash.as_bytes() {
        return Err(Error::WrongValidatorsHash);
    }
    let next_hash = state.next_validators.hash()?;
    if block.header.next_validators_hash.as_slice() != next_hash.as_bytes() {
        return Err(Error::WrongNextValidatorsHash);
    }
    let consensus_hash = hash_consensus_params(&state.consensus_params);
    if block.header.consensus_hash.as_slice() != consensus_hash.as_bytes() {
        return Err(Error::WrongConsensusHash);
    }
    match &block.last_commit {
        None => {
            if block.header.height != state.initial_height {
                return Err(Error::NilLastCommit);
            }
            if !block.header.last_commit_hash.is_empty() {
                return Err(Error::WrongLastCommitHash);
            }
        }
        Some(commit) => {
            if block.header.height == state.initial_height && !commit.signatures.is_empty() {
                return Err(Error::InitialCommitHasSignatures);
            }
            if block.header.last_commit_hash.as_slice() != commit.hash().as_bytes() {
                return Err(Error::WrongLastCommitHash);
            }
        }
    }
    if block.header.height == state.initial_height {
        if block.header.time != state.last_block_time {
            return Err(Error::WrongBlockTime);
        }
    } else if let Some(commit) = &block.last_commit {
        if block.header.time <= state.last_block_time {
            return Err(Error::BlockTimeNotIncreasing);
        }
        if block.header.time != median_time(commit, &state.last_validators) {
            return Err(Error::WrongBlockTime);
        }
    }
    let evidence_bytes = block.evidence.byte_size();
    if evidence_bytes > state.consensus_params.evidence.max_bytes {
        return Err(Error::EvidenceOverflow {
            max: state.consensus_params.evidence.max_bytes,
            got: evidence_bytes,
        });
    }
    Ok(())
}

fn last_commit_votes(state: &State, block: &Block) -> Vec<VoteInfo> {
    let Some(commit) = &block.last_commit else {
        return Vec::new();
    };
    if commit.signatures.len() != state.last_validators.validators().len() {
        return Vec::new();
    }
    state
        .last_validators
        .validators()
        .iter()
        .zip(commit.signatures.iter())
        .map(|(validator, signature)| VoteInfo {
            validator: Some(eld_tendermint_proto::abci::Validator {
                address: Bytes::copy_from_slice(&validator.address),
                power: validator.voting_power,
            }),
            signed_last_block: !signature.is_absent(),
        })
        .collect()
}

pub(crate) fn validator_updates(
    updates: &[ValidatorUpdate],
    params: &eld_tendermint_types::ConsensusParams,
) -> Result<Vec<Validator>, Error> {
    let mut parsed = Vec::with_capacity(updates.len());
    for update in updates {
        if update.power < 0 {
            return Err(Error::NegativeVotingPower);
        }
        let proto_key = update
            .pub_key
            .as_ref()
            .ok_or(Error::Types(eld_tendermint_types::Error::MissingPubKey))?;
        if update.power > 0 {
            let type_name = match &proto_key.sum {
                Some(eld_tendermint_proto::crypto::public_key::Sum::Ed25519(_)) => {
                    ABCI_PUBKEY_TYPE_ED25519
                }
                Some(eld_tendermint_proto::crypto::public_key::Sum::Secp256k1(_)) => {
                    eld_tendermint_types::ABCI_PUBKEY_TYPE_SECP256K1
                }
                None => {
                    return Err(Error::UnsupportedPubKeyType {
                        got: "absent".to_owned(),
                    });
                }
            };
            if !params
                .validator
                .pub_key_types
                .iter()
                .any(|allowed| allowed == type_name)
            {
                return Err(Error::UnsupportedPubKeyType {
                    got: type_name.to_owned(),
                });
            }
        }
        let pub_key = pub_key_from_proto(proto_key)
            .map_err(|err| Error::Types(eld_tendermint_types::Error::PubKey(err)))?;
        let mut validator = Validator::new(pub_key, update.power);
        validator.voting_power = update.power;
        parsed.push(validator);
    }
    Ok(parsed)
}

/// `ABCIResponsesResultsHash`. Events, log, info, and codespace are stripped
/// before the protobuf bytes are hashed. No transactions is the empty-tree hash.
fn results_hash(responses: &[ResponseDeliverTx]) -> Vec<u8> {
    let leaves: Vec<Vec<u8>> = responses
        .iter()
        .map(|response| {
            ResponseDeliverTx {
                code: response.code,
                data: response.data.clone(),
                log: String::new(),
                info: String::new(),
                gas_wanted: response.gas_wanted,
                gas_used: response.gas_used,
                events: Vec::new(),
                codespace: String::new(),
            }
            .encode_to_vec()
        })
        .collect();
    hash_from_byte_slices(&leaves).to_vec()
}
