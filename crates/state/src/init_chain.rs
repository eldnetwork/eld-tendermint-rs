//! First-start `InitChain` from `consensus/replay.go`.
//!
//! A missing `stateKey` is the signal. The response is saved before this returns,
//! so a later open does not call `InitChain` again.

use eld_tendermint_crypto::{hash_from_byte_slices, pub_key_to_proto};
use eld_tendermint_proto::abci::{
    BlockParams, ConsensusParams as AbciConsensusParams, RequestInitChain, ResponseInitChain,
    ValidatorUpdate,
};
use eld_tendermint_store::Db;
use eld_tendermint_types::{ConsensusParams, GenesisDoc, Level, ValidatorSet, log_line};
use prost::bytes::Bytes;

use crate::error::Error;
use crate::execution::validator_updates;
use crate::state::{State, make_genesis_state};
use crate::store::StateStore;

/// Loads `stateKey`, or builds genesis state, calls `init` once, and saves.
///
/// `init` is `InitChainSync`. It is not called when `stateKey` is already present.
/// A failure leaves `stateKey` unset.
///
/// Validator updates replace `next_validators` only. They become current when the
/// next block is applied. An empty `app_hash` keeps the genesis hash.
///
/// # Errors
///
/// Returns a genesis, ABCI, validator, or database error.
pub fn load_or_init_chain<D, F>(
    store: &StateStore<D>,
    genesis: &mut GenesisDoc,
    init: F,
) -> Result<State, Error>
where
    D: Db,
    F: FnOnce(RequestInitChain) -> Result<ResponseInitChain, Error>,
{
    if let Some(state) = store.load() {
        return Ok(state);
    }
    log_line(
        Level::Info,
        "consensus",
        "InitChain",
        &[("chain_id", genesis.chain_id.as_str())],
    );
    let state = make_genesis_state(genesis)?;
    let request = request_init_chain(genesis)?;
    let response = init(request)?;
    let state = apply_init_chain(state, &response)?;
    store.save(&state)?;
    Ok(state)
}

fn request_init_chain(genesis: &GenesisDoc) -> Result<RequestInitChain, Error> {
    let initial_height = if genesis.initial_height == 0 {
        1
    } else {
        genesis.initial_height
    };
    let params = genesis
        .consensus_params
        .clone()
        .unwrap_or_else(ConsensusParams::default_params);
    let validators = genesis
        .validators
        .iter()
        .map(|validator| ValidatorUpdate {
            pub_key: Some(pub_key_to_proto(&validator.pub_key)),
            power: validator.power,
        })
        .collect();
    let app_state_bytes = match &genesis.app_state {
        Some(app_state) => Bytes::from(
            serde_json::to_vec(app_state)
                .map_err(|err| Error::Types(eld_tendermint_types::Error::Json(err.to_string())))?,
        ),
        None => Bytes::new(),
    };
    Ok(RequestInitChain {
        time: Some(genesis.genesis_time.to_prost()),
        chain_id: genesis.chain_id.as_str().to_owned(),
        consensus_params: Some(abci_consensus_params(&params)),
        validators,
        app_state_bytes,
        initial_height,
    })
}

/// `TM2PB.ConsensusParams`. `version` stays unset. Go does not send it.
fn abci_consensus_params(params: &ConsensusParams) -> AbciConsensusParams {
    let proto = params.to_proto();
    AbciConsensusParams {
        block: Some(BlockParams {
            max_bytes: params.block.max_bytes,
            max_gas: params.block.max_gas,
        }),
        evidence: proto.evidence,
        validator: proto.validator,
        version: None,
    }
}

fn apply_init_chain(mut state: State, response: &ResponseInitChain) -> Result<State, Error> {
    if response.validators.is_empty() && state.validators.validators().is_empty() {
        return Err(Error::NilValidatorSet);
    }
    if !response.app_hash.is_empty() {
        state.app_hash = response.app_hash.to_vec();
    }
    if !response.validators.is_empty() {
        let updates = validator_updates(&response.validators, &state.consensus_params)?;
        let mut next_validators = ValidatorSet::new(updates)?;
        next_validators.increment_proposer_priority(1)?;
        state.next_validators = next_validators;
    }
    if let Some(update) = &response.consensus_params {
        state.consensus_params = state.consensus_params.update(update);
        state.consensus_params.validate()?;
        state.version.consensus.app = state.consensus_params.version.app_version;
    }
    state.last_results_hash = hash_from_byte_slices::<&[u8]>(&[]).to_vec();
    Ok(state)
}
