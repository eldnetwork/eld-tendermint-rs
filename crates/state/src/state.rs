//! `state.State` and `MakeGenesisState`.

use prost::Message;

use eld_tendermint_types::{
    BlockId, ChainId, ConsensusParams, ConsensusVersion, Time, Validator, ValidatorSet,
};

use crate::error::Error;
use eld_tendermint_types::GenesisDoc;

/// `version.TMCoreSemVer` for Tendermint 0.34.24.
pub const TM_CORE_SEMVER: &str = "0.34.24";

/// `state.Version`. Consensus app version is filled from the app during handshake;
/// genesis leaves it at 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateVersion {
    pub consensus: ConsensusVersion,
    pub software: String,
}

/// `state.State`. Fields are public the way Go's are. Mutate through `apply_block`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: StateVersion,
    pub chain_id: ChainId,
    pub initial_height: i64,
    pub last_block_height: i64,
    pub last_block_id: BlockId,
    pub last_block_time: Time,
    pub next_validators: ValidatorSet,
    pub validators: ValidatorSet,
    pub last_validators: ValidatorSet,
    pub last_height_validators_changed: i64,
    pub consensus_params: ConsensusParams,
    pub last_height_consensus_params_changed: i64,
    pub last_results_hash: Vec<u8>,
    pub app_hash: Vec<u8>,
}

/// `MakeGenesisState`.
///
/// `next_validators` is the genesis set incremented one step past `validators`.
/// Go is `NewValidatorSet` plus `CopyIncrementProposerPriority(1)`, not an identical copy.
///
/// # Errors
///
/// Returns a genesis or validator-set error.
pub fn make_genesis_state(genesis: &mut GenesisDoc) -> Result<State, Error> {
    genesis.validate_and_complete()?;
    let validators = genesis
        .validators
        .iter()
        .map(|validator| Validator::new(validator.pub_key, validator.power))
        .collect();
    let validators = ValidatorSet::new(validators)?;
    let mut next_validators = validators.copy();
    next_validators.increment_proposer_priority(1)?;
    let consensus_params = genesis
        .consensus_params
        .clone()
        .unwrap_or_else(ConsensusParams::default_params);
    Ok(State {
        version: StateVersion {
            consensus: ConsensusVersion::default(),
            software: TM_CORE_SEMVER.to_owned(),
        },
        chain_id: genesis.chain_id.clone(),
        initial_height: genesis.initial_height,
        last_block_height: 0,
        last_block_id: BlockId::default(),
        last_block_time: genesis.genesis_time,
        next_validators,
        validators,
        last_validators: ValidatorSet::default(),
        last_height_validators_changed: genesis.initial_height,
        consensus_params,
        last_height_consensus_params_changed: genesis.initial_height,
        last_results_hash: Vec::new(),
        app_hash: genesis.app_hash.clone(),
    })
}

impl State {
    /// Protobuf `tendermint.state.State`. `last_validators` is omitted when
    /// `last_block_height < 1`, matching `State.ToProto`.
    ///
    /// # Errors
    ///
    /// Returns a validator-set proto error.
    pub fn to_proto(&self) -> Result<eld_tendermint_proto::state::State, Error> {
        Ok(eld_tendermint_proto::state::State {
            version: Some(eld_tendermint_proto::state::Version {
                consensus: Some(eld_tendermint_proto::version::Consensus {
                    block: self.version.consensus.block,
                    app: self.version.consensus.app,
                }),
                software: self.version.software.clone(),
            }),
            chain_id: self.chain_id.as_str().to_owned(),
            initial_height: self.initial_height,
            last_block_height: self.last_block_height,
            last_block_id: Some(self.last_block_id.to_proto()),
            last_block_time: Some(self.last_block_time.to_prost()),
            next_validators: Some(self.next_validators.to_proto()?),
            validators: Some(self.validators.to_proto()?),
            last_validators: if self.last_block_height >= 1 {
                Some(self.last_validators.to_proto()?)
            } else {
                None
            },
            last_height_validators_changed: self.last_height_validators_changed,
            consensus_params: Some(self.consensus_params.to_proto()),
            last_height_consensus_params_changed: self.last_height_consensus_params_changed,
            last_results_hash: self.last_results_hash.clone(),
            app_hash: self.app_hash.clone(),
        })
    }

    /// `state.FromProto`.
    ///
    /// # Errors
    ///
    /// Returns a missing-field or validator-set error.
    pub fn try_from_proto(proto: &eld_tendermint_proto::state::State) -> Result<Self, Error> {
        let version = proto.version.as_ref();
        let consensus = version
            .and_then(|version| version.consensus)
            .unwrap_or_default();
        let validators = proto
            .validators
            .as_ref()
            .ok_or(Error::MissingValidators)
            .and_then(|set| ValidatorSet::try_from_proto(set).map_err(Error::from))?;
        let next_validators = proto
            .next_validators
            .as_ref()
            .ok_or(Error::MissingNextValidators)
            .and_then(|set| ValidatorSet::try_from_proto(set).map_err(Error::from))?;
        let last_validators = if proto.last_block_height >= 1 {
            proto
                .last_validators
                .as_ref()
                .ok_or(Error::MissingLastValidators)
                .and_then(|set| ValidatorSet::try_from_proto(set).map_err(Error::from))?
        } else {
            ValidatorSet::default()
        };
        let consensus_params = proto
            .consensus_params
            .as_ref()
            .ok_or(Error::MissingConsensusParams)
            .and_then(|params| ConsensusParams::try_from_proto(params).map_err(Error::from))?;
        let last_block_id = match &proto.last_block_id {
            Some(block_id) => BlockId::try_from_proto(block_id)?,
            None => BlockId::default(),
        };
        Ok(Self {
            version: StateVersion {
                consensus: ConsensusVersion {
                    block: consensus.block,
                    app: consensus.app,
                },
                software: version
                    .map(|version| version.software.clone())
                    .unwrap_or_default(),
            },
            chain_id: ChainId::new(proto.chain_id.clone()),
            initial_height: proto.initial_height,
            last_block_height: proto.last_block_height,
            last_block_id,
            last_block_time: Time::from_prost(proto.last_block_time.as_ref()),
            next_validators,
            validators,
            last_validators,
            last_height_validators_changed: proto.last_height_validators_changed,
            consensus_params,
            last_height_consensus_params_changed: proto.last_height_consensus_params_changed,
            last_results_hash: proto.last_results_hash.clone(),
            app_hash: proto.app_hash.clone(),
        })
    }

    pub(crate) fn encode(&self) -> Result<Vec<u8>, Error> {
        Ok(self.to_proto()?.encode_to_vec())
    }
}
