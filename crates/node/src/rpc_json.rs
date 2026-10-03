//! RPC JSON for the methods whose byte fields are standard base64.
//!
//! Field names are the Go `json` tags. Public keys stay the Amino envelope from
//! [`marshal_pub_key`](eld_tendermint_crypto::marshal_pub_key).

use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use eld_tendermint_crypto::{PubKey, marshal_pub_key};
use eld_tendermint_types::{BlockId, Header, Validator};

/// Standard base64, padding kept. An empty slice is `""`.
#[must_use]
pub(crate) fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

#[derive(Serialize)]
pub(crate) struct StatusResponse {
    pub node_info: NodeInfo,
    pub sync_info: SyncInfo,
    pub validator_info: ValidatorInfo,
}

#[derive(Serialize)]
pub(crate) struct NodeInfo {
    pub protocol_version: ProtocolVersion,
    pub id: String,
    pub listen_addr: String,
    pub network: String,
    pub version: String,
    pub channels: String,
    pub moniker: String,
    pub other: NodeInfoOther,
}

#[derive(Serialize)]
pub(crate) struct ProtocolVersion {
    pub p2p: u64,
    pub block: u64,
    pub app: u64,
}

#[derive(Serialize)]
pub(crate) struct NodeInfoOther {
    pub tx_index: String,
    pub rpc_address: String,
}

#[derive(Serialize)]
pub(crate) struct SyncInfo {
    pub latest_block_hash: String,
    pub latest_app_hash: String,
    pub latest_block_height: i64,
    pub latest_block_time: String,
    pub earliest_block_hash: String,
    pub earliest_app_hash: String,
    pub earliest_block_height: i64,
    pub earliest_block_time: String,
    pub catching_up: bool,
}

#[derive(Serialize)]
pub(crate) struct ValidatorInfo {
    pub address: String,
    pub pub_key: Value,
    pub voting_power: i64,
}

#[derive(Serialize)]
pub(crate) struct RpcValidator {
    pub address: String,
    pub pub_key: Option<Value>,
    pub voting_power: i64,
    pub proposer_priority: i64,
}

impl RpcValidator {
    pub(crate) fn from_validator(validator: &Validator) -> Self {
        Self {
            address: b64(&validator.address),
            pub_key: validator.pub_key.as_ref().and_then(amino_pub_key),
            voting_power: validator.voting_power,
            proposer_priority: validator.proposer_priority,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct RpcBlockId {
    pub hash: String,
    pub parts: RpcPartSetHeader,
}

#[derive(Serialize)]
pub(crate) struct RpcPartSetHeader {
    pub total: u32,
    pub hash: String,
}

impl RpcBlockId {
    pub(crate) fn from_block_id(block_id: &BlockId) -> Self {
        Self {
            hash: b64(&block_id.hash),
            parts: RpcPartSetHeader {
                total: block_id.part_set_header.total,
                hash: b64(&block_id.part_set_header.hash),
            },
        }
    }
}

/// Header fields. `height` stays a decimal string, matching the responses already served.
#[derive(Serialize)]
pub(crate) struct RpcHeader {
    pub version: RpcConsensusVersion,
    pub chain_id: String,
    pub height: String,
    pub time: String,
    pub last_block_id: RpcBlockId,
    pub last_commit_hash: String,
    pub data_hash: String,
    pub validators_hash: String,
    pub next_validators_hash: String,
    pub consensus_hash: String,
    pub app_hash: String,
    pub last_results_hash: String,
    pub evidence_hash: String,
    pub proposer_address: String,
}

#[derive(Serialize)]
pub(crate) struct RpcConsensusVersion {
    pub block: String,
    pub app: String,
}

impl RpcHeader {
    pub(crate) fn from_header(header: &Header) -> Self {
        Self {
            version: RpcConsensusVersion {
                block: header.version.block.to_string(),
                app: header.version.app.to_string(),
            },
            chain_id: header.chain_id.as_str().to_owned(),
            height: header.height.to_string(),
            time: header.time.to_rfc3339(),
            last_block_id: RpcBlockId::from_block_id(&header.last_block_id),
            last_commit_hash: b64(&header.last_commit_hash),
            data_hash: b64(&header.data_hash),
            validators_hash: b64(&header.validators_hash),
            next_validators_hash: b64(&header.next_validators_hash),
            consensus_hash: b64(&header.consensus_hash),
            app_hash: b64(&header.app_hash),
            last_results_hash: b64(&header.last_results_hash),
            evidence_hash: b64(&header.evidence_hash),
            proposer_address: b64(&header.proposer_address),
        }
    }
}

fn amino_pub_key(key: &PubKey) -> Option<Value> {
    serde_json::from_str(&marshal_pub_key(key)).ok()
}
