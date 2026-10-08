//! RPC JSON.
//!
//! `status` follows Tendermint 0.34 JSON-RPC: protocol versions, heights, and
//! voting power are decimal strings. Hashes and addresses that are Go
//! `bytes.HexBytes` (block IDs, header hashes, proposer and validator
//! addresses, transaction hashes in `broadcast_tx_*`, `tx`, `tx_search`, and Tx
//! events) are uppercase hex. Other byte fields stay standard base64. Field
//! names are the Go `json` tags. Public keys stay the Amino envelope from
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

/// Uppercase hex. An empty slice is `""`, which Tendermint uses for a missing hash.
#[must_use]
pub(crate) fn hex_upper(bytes: &[u8]) -> String {
    hex::encode(bytes).to_ascii_uppercase()
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
    pub p2p: String,
    pub block: String,
    pub app: String,
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
    pub latest_block_height: String,
    pub latest_block_time: String,
    pub earliest_block_hash: String,
    pub earliest_app_hash: String,
    pub earliest_block_height: String,
    pub earliest_block_time: String,
    pub catching_up: bool,
}

#[derive(Serialize)]
pub(crate) struct ValidatorInfo {
    pub address: String,
    pub pub_key: Value,
    pub voting_power: String,
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
            address: hex_upper(&validator.address),
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
            hash: hex_upper(&block_id.hash),
            parts: RpcPartSetHeader {
                total: block_id.part_set_header.total,
                hash: hex_upper(&block_id.part_set_header.hash),
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
            last_commit_hash: hex_upper(&header.last_commit_hash),
            data_hash: hex_upper(&header.data_hash),
            validators_hash: hex_upper(&header.validators_hash),
            next_validators_hash: hex_upper(&header.next_validators_hash),
            consensus_hash: hex_upper(&header.consensus_hash),
            app_hash: hex_upper(&header.app_hash),
            last_results_hash: hex_upper(&header.last_results_hash),
            evidence_hash: hex_upper(&header.evidence_hash),
            proposer_address: hex_upper(&header.proposer_address),
        }
    }
}

fn amino_pub_key(key: &PubKey) -> Option<Value> {
    serde_json::from_str(&marshal_pub_key(key)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_uses_tendermint_0_34_strings_and_hex() {
        let value = to_json(&StatusResponse {
            node_info: NodeInfo {
                protocol_version: ProtocolVersion {
                    p2p: "8".to_owned(),
                    block: "11".to_owned(),
                    app: "0".to_owned(),
                },
                id: "9ab26a12cbdbebbfaa962841ae782a5459b4d704".to_owned(),
                listen_addr: "tcp://0.0.0.0:26656".to_owned(),
                network: "eld-testnet-tempelhof".to_owned(),
                version: "0.34.24".to_owned(),
                channels: "3020212223004038".to_owned(),
                moniker: "tendermint-1".to_owned(),
                other: NodeInfoOther {
                    tx_index: "on".to_owned(),
                    rpc_address: "tcp://0.0.0.0:26657".to_owned(),
                },
            },
            sync_info: SyncInfo {
                latest_block_hash: hex_upper(&[0xab, 0xcd]),
                latest_app_hash: String::new(),
                latest_block_height: 12.to_string(),
                latest_block_time: "2026-10-06T03:57:37.135126836Z".to_owned(),
                earliest_block_hash: String::new(),
                earliest_app_hash: String::new(),
                earliest_block_height: 1.to_string(),
                earliest_block_time: "1970-01-01T00:00:00Z".to_owned(),
                catching_up: false,
            },
            validator_info: ValidatorInfo {
                address: hex_upper(&[
                    0x8b, 0x81, 0xcc, 0x2b, 0xa4, 0x1d, 0x4c, 0x29, 0xd0, 0xa0, 0xf9, 0x01, 0x23,
                    0x39, 0x69, 0x93, 0x49, 0x1c, 0x3e, 0xa6,
                ]),
                pub_key: serde_json::json!({
                    "type": "tendermint/PubKeyEd25519",
                    "value": "7EtfyZfBMTZ+rh7zHE+swgcH6roeoRYOvdFMoDW9+eU="
                }),
                voting_power: 1.to_string(),
            },
        });

        assert_eq!(value["node_info"]["protocol_version"]["p2p"], "8");
        assert_eq!(value["node_info"]["protocol_version"]["block"], "11");
        assert_eq!(value["node_info"]["protocol_version"]["app"], "0");
        assert_eq!(value["sync_info"]["latest_block_hash"], "ABCD");
        assert_eq!(value["sync_info"]["latest_app_hash"], "");
        assert_eq!(value["sync_info"]["latest_block_height"], "12");
        assert_eq!(value["sync_info"]["earliest_block_height"], "1");
        assert_eq!(
            value["validator_info"]["address"],
            "8B81CC2BA41D4C29D0A0F90123396993491C3EA6"
        );
        assert_eq!(value["validator_info"]["voting_power"], "1");
    }

    #[test]
    fn block_id_hex_empty_and_zero_hash() {
        let empty = to_json(&RpcBlockId::from_block_id(&BlockId::default()));
        assert_eq!(empty["hash"], "");

        let zeros = to_json(&RpcBlockId::from_block_id(&BlockId {
            hash: vec![0; 32],
            part_set_header: Default::default(),
        }));
        assert_eq!(zeros["hash"], "0".repeat(64));
    }
}
