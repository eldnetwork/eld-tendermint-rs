#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Proposal blocks reap only `MaxDataBytes`, not `Block.MaxBytes`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_consensus::Node;
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::{RequestCheckTx, ResponseCheckTx};
use eld_tendermint_state::make_genesis_state;
use eld_tendermint_types::{
    ChainId, ConsensusParams, GenesisDoc, GenesisValidator, Time, Tx, max_commit_bytes,
    max_data_bytes,
};
use prost::bytes::Bytes;

struct Check;

impl MempoolApp for Check {
    fn check_tx(&mut self, _: RequestCheckTx) -> ResponseCheckTx {
        ResponseCheckTx {
            code: 0,
            data: Bytes::new(),
            log: String::new(),
            info: String::new(),
            gas_wanted: 1,
            gas_used: 0,
            events: Vec::new(),
            codespace: String::new(),
            sender: String::new(),
            priority: 0,
            mempool_error: String::new(),
        }
    }
}

struct Exec;

impl eld_tendermint_state::App for Exec {
    fn begin_block(
        &mut self,
        _: eld_tendermint_proto::abci::RequestBeginBlock,
    ) -> Result<eld_tendermint_proto::abci::ResponseBeginBlock, eld_tendermint_state::Error> {
        Ok(eld_tendermint_proto::abci::ResponseBeginBlock { events: Vec::new() })
    }

    fn deliver_tx(
        &mut self,
        _: eld_tendermint_proto::abci::RequestDeliverTx,
    ) -> Result<eld_tendermint_proto::abci::ResponseDeliverTx, eld_tendermint_state::Error> {
        Ok(eld_tendermint_proto::abci::ResponseDeliverTx {
            code: 0,
            data: Bytes::new(),
            log: String::new(),
            info: String::new(),
            gas_wanted: 0,
            gas_used: 0,
            events: Vec::new(),
            codespace: String::new(),
        })
    }

    fn end_block(
        &mut self,
        _: eld_tendermint_proto::abci::RequestEndBlock,
    ) -> Result<eld_tendermint_proto::abci::ResponseEndBlock, eld_tendermint_state::Error> {
        Ok(eld_tendermint_proto::abci::ResponseEndBlock {
            validator_updates: Vec::new(),
            consensus_param_updates: None,
            events: Vec::new(),
        })
    }

    fn commit(
        &mut self,
    ) -> Result<eld_tendermint_proto::abci::ResponseCommit, eld_tendermint_state::Error> {
        Ok(eld_tendermint_proto::abci::ResponseCommit {
            data: Bytes::new(),
            retain_height: 0,
        })
    }
}

#[test]
fn initial_height_uses_genesis_time_and_vote_floor() {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-bft-time-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pv = FilePV::load_or_gen_file_pv(
        dir.join("priv_validator_key.json"),
        dir.join("priv_validator_state.json"),
    )
    .expect("file pv");
    let genesis_time = Time::from_unix_parts(4_000_000_000, 0);
    let mut genesis = GenesisDoc {
        genesis_time,
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: None,
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key: pv.get_pub_key(),
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    let state = make_genesis_state(&mut genesis).expect("genesis");
    let iota = state.consensus_params.block.time_iota_ms;
    let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    let before = Time::now();
    let node = Node::start(ConsensusConfig::test_config(), pv, state, mempool, Exec).expect("node");
    let after = Time::now();
    let block = node.proposal_block().expect("block");
    assert_eq!(block.header.time, genesis_time);
    let proposal = node.queued_proposal().expect("proposal");
    assert!(proposal.timestamp >= before && proposal.timestamp <= after);
    assert_ne!(proposal.timestamp, block.header.time);
    let prevote = node.queued_prevote().expect("prevote");
    assert_eq!(prevote.timestamp, genesis_time.add_millis(iota));
}

#[test]
fn proposal_trims_txs_to_max_data_bytes() {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-proposal-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pv = FilePV::load_or_gen_file_pv(
        dir.join("priv_validator_key.json"),
        dir.join("priv_validator_state.json"),
    )
    .expect("file pv");

    // One validator: commit reservation is 205. Leave room for one ~200-byte tx, not three.
    let max_bytes = max_commit_bytes(1) + 11 + 626 + 300;
    let mut params = ConsensusParams::default_params();
    params.block.max_bytes = max_bytes;
    params.evidence.max_bytes = 0;
    let mut genesis = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: Some(params),
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key: pv.get_pub_key(),
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    let state = make_genesis_state(&mut genesis).expect("genesis");
    let budget = max_data_bytes(max_bytes, 0, 1).expect("data budget");
    assert_eq!(budget, 300);

    let mut mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    for index in 0..3 {
        let mut bytes = vec![u8::try_from(index).unwrap_or(0); 200];
        bytes[0] = u8::try_from(index).unwrap_or(0);
        mempool
            .check_tx(&Tx::new(bytes))
            .expect("tx fits the mempool");
    }
    assert_eq!(mempool.size(), 3);

    let node = Node::start(ConsensusConfig::test_config(), pv, state, mempool, Exec).expect("node");
    let block = node.proposal_block().expect("proposal block");
    assert_eq!(
        block.data.as_slice().len(),
        1,
        "only one tx fits MaxDataBytes"
    );
    assert_eq!(node.mempool().lock().expect("mempool").size(), 3);
}
