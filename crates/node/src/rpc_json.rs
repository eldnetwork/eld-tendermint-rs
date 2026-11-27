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
use eld_tendermint_types::{
    BlockId, ConsensusParams, Header, SummaryHeader, Validator, ValidatorSet,
};

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

/// Summary header. Heights are decimal strings. Hashes are uppercase hex.
#[derive(Serialize)]
pub(crate) struct RpcSummaryHeader {
    pub end_height: String,
    pub summary_hash: String,
    pub last_block_id: RpcBlockId,
    pub last_results_hash: String,
    pub app_hash: String,
    pub body_hash: String,
    pub validators_hash: String,
    pub next_validators_hash: String,
    pub consensus_hash: String,
    pub last_height_validators_changed: String,
    pub last_height_consensus_params_changed: String,
    pub validators: RpcValidatorSet,
    pub next_validators: RpcValidatorSet,
    pub consensus_params: RpcConsensusParams,
}

#[derive(Serialize)]
pub(crate) struct RpcValidatorSet {
    pub validators: Vec<RpcValidator>,
    pub proposer: Option<RpcValidator>,
}

#[derive(Serialize)]
pub(crate) struct RpcConsensusParams {
    pub block: RpcBlockParams,
    pub evidence: RpcEvidenceParams,
    pub validator: RpcValidatorParams,
    pub version: RpcVersionParams,
}

#[derive(Serialize)]
pub(crate) struct RpcBlockParams {
    pub max_bytes: String,
    pub max_gas: String,
    pub time_iota_ms: String,
}

#[derive(Serialize)]
pub(crate) struct RpcEvidenceParams {
    pub max_age_num_blocks: String,
    pub max_age_duration: String,
    pub max_bytes: String,
}

#[derive(Serialize)]
pub(crate) struct RpcValidatorParams {
    pub pub_key_types: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct RpcVersionParams {
    pub app_version: String,
}

impl RpcSummaryHeader {
    pub(crate) fn from_summary_header(header: &SummaryHeader) -> Self {
        Self {
            end_height: header.end_height.to_string(),
            summary_hash: hex_upper(header.summary_hash.as_bytes()),
            last_block_id: RpcBlockId::from_block_id(&header.last_block_id),
            last_results_hash: hex_upper(header.last_results_hash.as_bytes()),
            app_hash: hex_upper(header.app_hash.as_bytes()),
            body_hash: hex_upper(header.body_hash.as_bytes()),
            validators_hash: hex_upper(header.validators_hash.as_bytes()),
            next_validators_hash: hex_upper(header.next_validators_hash.as_bytes()),
            consensus_hash: hex_upper(header.consensus_hash.as_bytes()),
            last_height_validators_changed: header.last_height_validators_changed.to_string(),
            last_height_consensus_params_changed: header
                .last_height_consensus_params_changed
                .to_string(),
            validators: RpcValidatorSet::from_set(&header.validators),
            next_validators: RpcValidatorSet::from_set(&header.next_validators),
            consensus_params: RpcConsensusParams::from_params(&header.consensus_params),
        }
    }
}

impl RpcValidatorSet {
    fn from_set(set: &ValidatorSet) -> Self {
        Self {
            validators: set
                .validators()
                .iter()
                .map(RpcValidator::from_validator)
                .collect(),
            proposer: set.proposer().map(RpcValidator::from_validator),
        }
    }
}

impl RpcConsensusParams {
    fn from_params(params: &ConsensusParams) -> Self {
        Self {
            block: RpcBlockParams {
                max_bytes: params.block.max_bytes.to_string(),
                max_gas: params.block.max_gas.to_string(),
                time_iota_ms: params.block.time_iota_ms.to_string(),
            },
            evidence: RpcEvidenceParams {
                max_age_num_blocks: params.evidence.max_age_num_blocks.to_string(),
                max_age_duration: params.evidence.max_age_duration.total_nanos().to_string(),
                max_bytes: params.evidence.max_bytes.to_string(),
            },
            validator: RpcValidatorParams {
                pub_key_types: params.validator.pub_key_types.clone(),
            },
            version: RpcVersionParams {
                app_version: params.version.app_version.to_string(),
            },
        }
    }
}

fn amino_pub_key(key: &PubKey) -> Option<Value> {
    serde_json::from_str(&marshal_pub_key(key)).ok()
}

#[cfg(test)]
mod tests {
    use eld_tendermint_crypto::{sum, unmarshal_pub_key};
    use eld_tendermint_types::{
        ConsensusParams, Hash, PartSetHeader, SummaryHeader, SummaryInput, Validator, ValidatorSet,
    };

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

    #[test]
    fn summary_header_json_uses_string_heights_and_uppercase_hex() {
        let header = sample_summary_header();
        let value = to_json(&RpcSummaryHeader::from_summary_header(&header));

        assert!(value.get("body").is_none());
        assert_eq!(value["end_height"], "10");
        assert_eq!(value["last_height_validators_changed"], "1");
        assert_eq!(value["last_height_consensus_params_changed"], "1");
        assert_eq!(
            value["summary_hash"],
            "24ADB8A1F5355DF7A0CF13B47F080F87A894A097D586564D2771BE81DF4BF505"
        );
        assert_eq!(
            value["body_hash"],
            "A606345E926BC749571988564AA8D506FFB506D5B0E40A2BFD6B819594A74146"
        );
        assert_eq!(value["app_hash"], hex_upper(header.app_hash.as_bytes()));
        assert_eq!(
            value["last_results_hash"],
            hex_upper(header.last_results_hash.as_bytes())
        );
        assert_eq!(
            value["validators_hash"],
            hex_upper(header.validators_hash.as_bytes())
        );
        assert_eq!(
            value["next_validators_hash"],
            hex_upper(header.next_validators_hash.as_bytes())
        );
        assert_eq!(
            value["consensus_hash"],
            hex_upper(header.consensus_hash.as_bytes())
        );
        assert_eq!(
            value["last_block_id"]["hash"],
            hex_upper(&header.last_block_id.hash)
        );
        assert_eq!(value["last_block_id"]["parts"]["total"], 1);

        assert_eq!(
            value["validators"]["validators"].as_array().map(Vec::len),
            Some(1)
        );
        let validator = &value["validators"]["validators"][0];
        assert_eq!(validator["voting_power"], 10);
        let address = validator["address"].as_str().unwrap();
        assert_eq!(address.to_ascii_uppercase(), address);
        assert_eq!(value["validators"]["proposer"]["address"], address);
        assert_eq!(
            value["next_validators"]["proposer"]["address"],
            value["next_validators"]["validators"][0]["address"]
        );

        assert_eq!(value["consensus_params"]["block"]["max_bytes"], "22020096");
        assert_eq!(value["consensus_params"]["block"]["max_gas"], "-1");
        assert_eq!(value["consensus_params"]["block"]["time_iota_ms"], "1000");
        assert_eq!(
            value["consensus_params"]["evidence"]["max_age_num_blocks"],
            "100000"
        );
        assert_eq!(
            value["consensus_params"]["evidence"]["max_age_duration"],
            "172800000000000"
        );
        assert_eq!(
            value["consensus_params"]["evidence"]["max_bytes"],
            "1048576"
        );
        assert_eq!(
            value["consensus_params"]["validator"]["pub_key_types"][0],
            "ed25519"
        );
        assert_eq!(value["consensus_params"]["version"]["app_version"], "0");
    }

    fn sample_summary_header() -> SummaryHeader {
        const PUB_JSON: &str = concat!(
            r#"{"type":"tendermint/PubKeyEd25519","value":""#,
            "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=",
            r#""}"#,
        );
        let pub_key = unmarshal_pub_key(PUB_JSON).unwrap();
        let validators = ValidatorSet::new(vec![Validator::new(pub_key, 10)]).unwrap();
        let mut next_validators = validators.copy();
        next_validators.increment_proposer_priority(1).unwrap();
        SummaryHeader::build(
            SummaryInput {
                end_height: 10,
                last_block_id: BlockId {
                    hash: sum(b"block").to_vec(),
                    part_set_header: PartSetHeader {
                        total: 1,
                        hash: sum(b"parts").to_vec(),
                    },
                },
                last_results_hash: Hash::from_array(sum(b"results")),
                app_hash: Hash::from_array(sum(b"app")),
                last_height_validators_changed: 1,
                last_height_consensus_params_changed: 1,
                validators,
                next_validators,
                consensus_params: ConsensusParams::default_params(),
            },
            b"compressed-snapshot",
        )
        .unwrap()
    }
}
