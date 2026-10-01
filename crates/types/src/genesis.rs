//! `types.GenesisDoc` JSON and `ValidateAndComplete`.

use eld_tendermint_crypto::{PubKey, marshal_pub_key, unmarshal_pub_key};
use serde::Deserialize;
use serde::de::{self, Deserializer};
use serde_json::{Map, Value};

use crate::time::Time;
use crate::{ChainId, ConsensusParams, Error, Hash, Validator, ValidatorSet};

/// One validator in a genesis file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenesisValidator {
    pub address: Vec<u8>,
    pub pub_key: PubKey,
    pub power: i64,
    pub name: String,
}

/// `types.GenesisDoc`. There is no `ValidateBasic`; use [`Self::validate_and_complete`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenesisDoc {
    pub genesis_time: Time,
    pub chain_id: ChainId,
    pub initial_height: i64,
    pub consensus_params: Option<ConsensusParams>,
    pub validators: Vec<GenesisValidator>,
    pub app_hash: Vec<u8>,
    pub app_state: Option<Value>,
}

impl GenesisDoc {
    /// `GenesisDocFromJSON`: parse Amino JSON, then [`Self::validate_and_complete`].
    ///
    /// # Errors
    ///
    /// Returns a JSON error or a validation error from [`Self::validate_and_complete`].
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let mut doc = parse_genesis(bytes)?;
        doc.validate_and_complete()?;
        Ok(doc)
    }

    /// Fills default consensus params, validator addresses, initial height, and genesis time.
    ///
    /// # Errors
    ///
    /// Returns an error when the chain id, height, params, voting power, or addresses are invalid.
    pub fn validate_and_complete(&mut self) -> Result<(), Error> {
        self.chain_id.validate_genesis()?;
        if self.initial_height < 0 {
            return Err(Error::NegativeHeight);
        }
        if self.initial_height == 0 {
            self.initial_height = 1;
        }
        match &self.consensus_params {
            None => self.consensus_params = Some(ConsensusParams::default_params()),
            Some(params) => params.validate()?,
        }
        for validator in &mut self.validators {
            if validator.power == 0 {
                return Err(Error::ZeroVotingPower);
            }
            let derived = validator.pub_key.address();
            if validator.address.is_empty() {
                validator.address = derived.to_vec();
            } else if validator.address.as_slice() != derived.as_slice() {
                return Err(Error::AddressMismatch);
            }
        }
        if self.genesis_time.is_zero() {
            self.genesis_time = Time::now();
        }
        Ok(())
    }

    /// `GenesisDoc.ValidatorHash`.
    ///
    /// An empty validator list is an error. Go's `NewValidatorSet` panics in that case.
    ///
    /// # Errors
    ///
    /// Returns [`Error::EmptyValidatorSet`] or a validator-set construction error.
    pub fn validator_hash(&self) -> Result<Hash, Error> {
        let validators = self
            .validators
            .iter()
            .map(|validator| Validator::new(validator.pub_key, validator.power))
            .collect();
        ValidatorSet::new(validators)?.hash()
    }

    /// Compact JSON in Go `libs/json` shape: int64 fields are decimal strings, hashes are
    /// uppercase hex, and the public key is an Amino envelope.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut doc = Map::new();
        doc.insert(
            "genesis_time".to_owned(),
            Value::String(self.genesis_time.to_rfc3339()),
        );
        doc.insert(
            "chain_id".to_owned(),
            Value::String(self.chain_id.as_str().to_owned()),
        );
        doc.insert(
            "initial_height".to_owned(),
            Value::String(self.initial_height.to_string()),
        );
        if let Some(params) = &self.consensus_params {
            doc.insert("consensus_params".to_owned(), consensus_params_json(params));
        }
        if !self.validators.is_empty() {
            let validators = self.validators.iter().map(genesis_validator_json).collect();
            doc.insert("validators".to_owned(), Value::Array(validators));
        }
        doc.insert(
            "app_hash".to_owned(),
            Value::String(hex::encode_upper(&self.app_hash)),
        );
        if let Some(app_state) = &self.app_state {
            doc.insert("app_state".to_owned(), app_state.clone());
        }
        Value::Object(doc).to_string()
    }
}

fn genesis_validator_json(validator: &GenesisValidator) -> Value {
    let pub_key = serde_json::from_str::<Value>(&marshal_pub_key(&validator.pub_key))
        .expect("Amino public key JSON is valid");
    let mut object = Map::new();
    object.insert(
        "address".to_owned(),
        Value::String(hex::encode_upper(&validator.address)),
    );
    object.insert("pub_key".to_owned(), pub_key);
    object.insert(
        "power".to_owned(),
        Value::String(validator.power.to_string()),
    );
    object.insert("name".to_owned(), Value::String(validator.name.clone()));
    Value::Object(object)
}

fn consensus_params_json(params: &ConsensusParams) -> Value {
    let mut block = Map::new();
    insert_i64(&mut block, "max_bytes", params.block.max_bytes);
    insert_i64(&mut block, "max_gas", params.block.max_gas);
    insert_i64(&mut block, "time_iota_ms", params.block.time_iota_ms);

    let mut evidence = Map::new();
    insert_i64(
        &mut evidence,
        "max_age_num_blocks",
        params.evidence.max_age_num_blocks,
    );
    evidence.insert(
        "max_age_duration".to_owned(),
        Value::String(params.evidence.max_age_duration.total_nanos().to_string()),
    );
    insert_i64(&mut evidence, "max_bytes", params.evidence.max_bytes);

    let mut validator = Map::new();
    if !params.validator.pub_key_types.is_empty() {
        let types = params
            .validator
            .pub_key_types
            .iter()
            .cloned()
            .map(Value::String)
            .collect();
        validator.insert("pub_key_types".to_owned(), Value::Array(types));
    }

    let mut version = Map::new();
    if params.version.app_version != 0 {
        version.insert(
            "app_version".to_owned(),
            Value::String(params.version.app_version.to_string()),
        );
    }

    let mut object = Map::new();
    object.insert("block".to_owned(), Value::Object(block));
    object.insert("evidence".to_owned(), Value::Object(evidence));
    object.insert("validator".to_owned(), Value::Object(validator));
    object.insert("version".to_owned(), Value::Object(version));
    Value::Object(object)
}

fn insert_i64(map: &mut Map<String, Value>, key: &str, value: i64) {
    if value != 0 {
        map.insert(key.to_owned(), Value::String(value.to_string()));
    }
}

fn parse_genesis(bytes: &[u8]) -> Result<GenesisDoc, Error> {
    let raw: RawGenesis =
        serde_json::from_slice(bytes).map_err(|err| Error::Json(err.to_string()))?;
    let genesis_time = match raw.genesis_time.as_deref() {
        None => Time::GO_ZERO,
        Some(text) => Time::parse_rfc3339(text)?,
    };
    let app_hash = match raw.app_hash.as_deref() {
        None | Some("") => Vec::new(),
        Some(text) => hex::decode(text).map_err(|_| Error::InvalidHex)?,
    };
    let mut validators = Vec::new();
    for validator in raw.validators.unwrap_or_default() {
        let Some(pub_key_json) = validator.pub_key else {
            return Err(Error::MissingPubKey);
        };
        let encoded =
            serde_json::to_string(&pub_key_json).map_err(|err| Error::Json(err.to_string()))?;
        let pub_key = unmarshal_pub_key(&encoded).map_err(Error::PubKey)?;
        let address = match validator.address.as_deref() {
            None | Some("") => Vec::new(),
            Some(text) => hex::decode(text).map_err(|_| Error::InvalidHex)?,
        };
        validators.push(GenesisValidator {
            address,
            pub_key,
            power: validator.power.unwrap_or(0),
            name: validator.name,
        });
    }
    let consensus_params = match raw.consensus_params {
        None => None,
        Some(params) => Some(params.into_params()?),
    };
    Ok(GenesisDoc {
        genesis_time,
        chain_id: ChainId::new(raw.chain_id),
        initial_height: raw.initial_height.unwrap_or(0),
        consensus_params,
        validators,
        app_hash,
        app_state: raw.app_state,
    })
}

#[derive(Deserialize)]
struct RawGenesis {
    #[serde(default)]
    genesis_time: Option<String>,
    #[serde(default)]
    chain_id: String,
    #[serde(default, deserialize_with = "de_opt_i64")]
    initial_height: Option<i64>,
    #[serde(default)]
    consensus_params: Option<RawConsensusParams>,
    #[serde(default)]
    validators: Option<Vec<RawGenesisValidator>>,
    #[serde(default)]
    app_hash: Option<String>,
    #[serde(default)]
    app_state: Option<Value>,
}

#[derive(Deserialize)]
struct RawGenesisValidator {
    #[serde(default)]
    address: Option<String>,
    pub_key: Option<Value>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    power: Option<i64>,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RawConsensusParams {
    #[serde(default)]
    block: RawBlockParams,
    #[serde(default)]
    evidence: RawEvidenceParams,
    #[serde(default)]
    validator: RawValidatorParams,
    #[serde(default)]
    version: RawVersionParams,
}

impl RawConsensusParams {
    fn into_params(self) -> Result<ConsensusParams, Error> {
        Ok(ConsensusParams {
            block: crate::BlockParams {
                max_bytes: self.block.max_bytes.unwrap_or(0),
                max_gas: self.block.max_gas.unwrap_or(0),
                time_iota_ms: self.block.time_iota_ms.unwrap_or(0),
            },
            evidence: crate::EvidenceParams {
                max_age_num_blocks: self.evidence.max_age_num_blocks.unwrap_or(0),
                max_age_duration: duration_from_json(self.evidence.max_age_duration.as_deref())?,
                max_bytes: self.evidence.max_bytes.unwrap_or(0),
            },
            validator: crate::ValidatorParams {
                pub_key_types: self.validator.pub_key_types.unwrap_or_default(),
            },
            version: crate::VersionParams {
                app_version: self.version.app_version.unwrap_or(0),
            },
        })
    }
}

#[derive(Deserialize, Default)]
struct RawBlockParams {
    #[serde(default, deserialize_with = "de_opt_i64")]
    max_bytes: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    max_gas: Option<i64>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    time_iota_ms: Option<i64>,
}

#[derive(Deserialize, Default)]
struct RawEvidenceParams {
    #[serde(default, deserialize_with = "de_opt_i64")]
    max_age_num_blocks: Option<i64>,
    #[serde(default)]
    max_age_duration: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i64")]
    max_bytes: Option<i64>,
}

#[derive(Deserialize, Default)]
struct RawValidatorParams {
    #[serde(default)]
    pub_key_types: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
struct RawVersionParams {
    #[serde(default, deserialize_with = "de_opt_u64")]
    app_version: Option<u64>,
}

fn duration_from_json(text: Option<&str>) -> Result<crate::Duration, Error> {
    let Some(text) = text else {
        return Ok(crate::Duration {
            seconds: 0,
            nanos: 0,
        });
    };
    let total: i128 = text.parse().map_err(|_| Error::InvalidInteger)?;
    crate::Duration::from_nanos(total)
}

fn de_opt_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => text.parse::<i64>().map(Some).map_err(de::Error::custom),
        Some(_) => Err(de::Error::custom("expected a string integer")),
    }
}

fn de_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => text.parse::<u64>().map(Some).map_err(de::Error::custom),
        Some(_) => Err(de::Error::custom("expected a string integer")),
    }
}
