//! Summary updates after a commit.
//!
//! [`crate::Node`] boot passes the execution app to [`SummaryManager::new`].
//! [`SummaryManager::update_summary`] asks the app for the frontier snapshot and
//! stores the summary header and body.

use std::sync::Arc;

use eld_tendermint_proto::abci::{RequestQuery, ResponseQuery};
use eld_tendermint_state::State;
use eld_tendermint_store::{BlockStore, Db};
use eld_tendermint_types::{
    BlockId, Hash, Level, SummaryBody, SummaryHeader, SummaryInput, log_line,
};
use prost::Message;

const FRONTIER_APPSTATE_PATH: &str = "/frontier_appstate";
const SNAPSHOT_QUERY_TOKEN_KEY: &str = "ELD_SNAPSHOT_QUERY_TOKEN";

/// Runs one ABCI query. The node crate implements this on `AbciApp`.
///
/// Consensus cannot call `AbciApp` directly: the node crate already depends on this one.
pub trait SummaryQuery: Send + Sync {
    /// # Errors
    ///
    /// Returns the socket error as text. The default reports that no query socket is connected.
    fn query(&self, _request: RequestQuery) -> Result<ResponseQuery, String> {
        Err("summary query is not connected".to_owned())
    }
}

/// Holds the summary update for one node.
pub struct SummaryManager {
    app: Arc<dyn SummaryQuery>,
}

impl SummaryManager {
    /// Creates the manager. Does not send a request.
    #[must_use]
    pub fn new(app: impl SummaryQuery + 'static) -> Self {
        Self { app: Arc::new(app) }
    }

    /// Asks the app for the snapshot at `frontier` and stores the summary.
    ///
    /// `block_id` is block 1. The committed height has to be `frontier`. A query
    /// error, a non-zero code, or an empty snapshot is logged and not stored.
    pub fn update_summary<D: Db>(
        &self,
        frontier: i64,
        state: &State,
        block_id: BlockId,
        block_store: &BlockStore<D>,
    ) {
        let token = match std::env::var(SNAPSHOT_QUERY_TOKEN_KEY) {
            Ok(token) if !token.is_empty() => token,
            Ok(_) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "ELD_SNAPSHOT_QUERY_TOKEN is empty",
                    &[],
                );
                return;
            }
            Err(_) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "ELD_SNAPSHOT_QUERY_TOKEN is unset",
                    &[],
                );
                return;
            }
        };
        self.write_summary(&token, frontier, state, block_id, block_store);
    }

    fn write_summary<D: Db>(
        &self,
        token: &str,
        frontier: i64,
        state: &State,
        block_id: BlockId,
        block_store: &BlockStore<D>,
    ) {
        if state.last_block_height != frontier {
            log_line(
                Level::Error,
                "consensus",
                "frontier does not match the committed block",
                &[
                    ("frontier", frontier.to_string().as_str()),
                    ("height", state.last_block_height.to_string().as_str()),
                ],
            );
            return;
        }
        let mut data = token.as_bytes().to_vec();
        data.extend_from_slice(&frontier.to_le_bytes());
        let request = RequestQuery {
            data: data.into(),
            path: FRONTIER_APPSTATE_PATH.to_owned(),
            height: 0,
            prove: false,
        };
        let response = match self.app.query(request) {
            Ok(response) => response,
            Err(err) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "frontier app state query failed",
                    &[
                        ("frontier", frontier.to_string().as_str()),
                        ("err", err.as_str()),
                    ],
                );
                return;
            }
        };
        log_line(
            Level::Info,
            "consensus",
            "frontier app state response",
            &[
                ("frontier", frontier.to_string().as_str()),
                ("code", response.code.to_string().as_str()),
                ("bytes", response.value.len().to_string().as_str()),
                ("log", response.log.as_str()),
            ],
        );
        if response.code != 0 || response.value.is_empty() {
            return;
        }
        let body = match summary_body(&response.value) {
            Ok(body) => body,
            Err(err) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "frontier app state is not a summary body",
                    &[
                        ("frontier", frontier.to_string().as_str()),
                        ("err", err.to_string().as_str()),
                    ],
                );
                return;
            }
        };
        if body.state_snapshot.is_empty() {
            log_line(
                Level::Error,
                "consensus",
                "frontier app state snapshot is empty",
                &[("frontier", frontier.to_string().as_str())],
            );
            return;
        }
        let header = match summary_header(frontier, state, block_id, &body.state_snapshot) {
            Ok(header) => header,
            Err(err) => {
                log_line(
                    Level::Error,
                    "consensus",
                    "summary header was not built",
                    &[
                        ("frontier", frontier.to_string().as_str()),
                        ("err", err.to_string().as_str()),
                    ],
                );
                return;
            }
        };
        if let Err(err) = block_store.save_summary(&header, &body) {
            log_line(
                Level::Error,
                "consensus",
                "summary was not saved",
                &[
                    ("frontier", frontier.to_string().as_str()),
                    ("err", err.to_string().as_str()),
                ],
            );
            return;
        }
        log_line(
            Level::Info,
            "consensus",
            "saved summary",
            &[
                ("frontier", frontier.to_string().as_str()),
                ("bytes", response.value.len().to_string().as_str()),
            ],
        );
    }
}

fn summary_header(
    frontier: i64,
    state: &State,
    block_id: BlockId,
    snapshot: &[u8],
) -> Result<SummaryHeader, eld_tendermint_types::Error> {
    let last_results_hash = Hash::from_bytes(&state.last_results_hash)?;
    let app_hash = Hash::from_bytes(&state.app_hash)?;
    SummaryHeader::build(
        SummaryInput {
            end_height: frontier,
            last_block_id: block_id,
            last_results_hash,
            app_hash,
            last_height_validators_changed: state.last_height_validators_changed,
            last_height_consensus_params_changed: state.last_height_consensus_params_changed,
            validators: state.validators.copy(),
            next_validators: state.next_validators.copy(),
            consensus_params: state.consensus_params.clone(),
        },
        snapshot,
    )
}

fn summary_body(bytes: &[u8]) -> Result<SummaryBody, prost::DecodeError> {
    let proto = eld_tendermint_proto::types::SummaryBody::decode(bytes)?;
    Ok(SummaryBody::from_proto(&proto))
}

#[cfg(test)]
mod tests {
    use eld_tendermint_crypto::{PubKey, sum};
    use eld_tendermint_state::make_genesis_state;
    use eld_tendermint_store::{BlockStore, MemDb};
    use eld_tendermint_types::{ChainId, GenesisDoc, GenesisValidator, PartSetHeader, Time};

    use super::*;

    struct Reply {
        code: u32,
        value: Vec<u8>,
    }

    impl SummaryQuery for Reply {
        fn query(&self, _: RequestQuery) -> Result<ResponseQuery, String> {
            Ok(ResponseQuery {
                code: self.code,
                log: String::new(),
                info: String::new(),
                index: 0,
                key: prost::bytes::Bytes::new(),
                value: self.value.clone().into(),
                proof_ops: None,
                height: 0,
                codespace: String::new(),
            })
        }
    }

    fn committed_state(height: i64) -> State {
        let mut genesis = GenesisDoc {
            genesis_time: Time::from_unix_parts(1_600_000_000, 0),
            chain_id: ChainId::new("test-chain"),
            initial_height: 1,
            consensus_params: None,
            validators: vec![GenesisValidator {
                address: Vec::new(),
                pub_key: PubKey::from_bytes(&[0x11; 32]).expect("pubkey"),
                power: 10,
                name: String::new(),
            }],
            app_hash: vec![0x22; 32],
            app_state: None,
        };
        let mut state = make_genesis_state(&mut genesis).expect("genesis");
        state.last_block_height = height;
        state.last_results_hash = sum(b"results").to_vec();
        state.app_hash = sum(b"app").to_vec();
        state
    }

    fn block_one() -> BlockId {
        BlockId {
            hash: sum(b"block-1").to_vec(),
            part_set_header: PartSetHeader {
                total: 1,
                hash: sum(b"parts").to_vec(),
            },
        }
    }

    fn encoded_body(snapshot: &[u8]) -> Vec<u8> {
        SummaryBody {
            state_snapshot: snapshot.to_vec(),
        }
        .to_proto()
        .encode_to_vec()
    }

    #[test]
    fn update_summary_saves_block_one_id_and_replaces_the_snapshot() {
        let store = BlockStore::new(MemDb::new());
        let block_id = block_one();
        SummaryManager::new(Reply {
            code: 0,
            value: encoded_body(b"snapshot-one"),
        })
        .write_summary("token", 4, &committed_state(4), block_id.clone(), &store);
        let header = store.load_summary_header().expect("header");
        let body = store.load_summary_body().expect("body");
        assert_eq!(header.end_height, 4);
        assert_eq!(header.last_block_id, block_id);
        assert_eq!(body.state_snapshot, b"snapshot-one");
        assert_eq!(header.body_hash, body.hash());

        SummaryManager::new(Reply {
            code: 0,
            value: encoded_body(b"snapshot-two"),
        })
        .write_summary("token", 9, &committed_state(9), block_id.clone(), &store);
        let header = store.load_summary_header().expect("replaced header");
        let body = store.load_summary_body().expect("replaced body");
        assert_eq!(header.end_height, 9);
        assert_eq!(header.last_block_id, block_id);
        assert_eq!(body.state_snapshot, b"snapshot-two");
    }

    #[test]
    fn update_summary_does_not_save_a_rejected_snapshot() {
        let store = BlockStore::new(MemDb::new());
        let block_id = block_one();
        SummaryManager::new(Reply {
            code: 4,
            value: encoded_body(b"nope"),
        })
        .write_summary("token", 4, &committed_state(4), block_id.clone(), &store);
        assert!(store.load_summary_header().is_none());
        assert!(store.load_summary_body().is_none());

        SummaryManager::new(Reply {
            code: 0,
            value: Vec::new(),
        })
        .write_summary("token", 4, &committed_state(4), block_id.clone(), &store);
        assert!(store.load_summary_header().is_none());

        SummaryManager::new(Reply {
            code: 0,
            value: encoded_body(b"snapshot"),
        })
        .write_summary("token", 4, &committed_state(3), block_id, &store);
        assert!(store.load_summary_header().is_none());
        assert!(store.load_summary_body().is_none());
    }
}
