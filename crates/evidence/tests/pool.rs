//! A duplicate vote is proposed once, then omitted after commit.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_consensus::{Group, Node};
use eld_tendermint_evidence::Pool;
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseEndBlock,
};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::{App as ExecApp, make_genesis_state};
use eld_tendermint_store::MemDb;
use eld_tendermint_types::{
    BlockId, ChainId, DuplicateVoteEvidence, EvidenceList, GenesisDoc, GenesisValidator,
    PartSetHeader, Time, Vote,
};
use prost::bytes::Bytes;

struct Exec;

impl ExecApp for Exec {
    fn begin_block(
        &mut self,
        _: RequestBeginBlock,
    ) -> Result<ResponseBeginBlock, eld_tendermint_state::Error> {
        Ok(ResponseBeginBlock { events: Vec::new() })
    }

    fn deliver_tx(
        &mut self,
        _: RequestDeliverTx,
    ) -> Result<ResponseDeliverTx, eld_tendermint_state::Error> {
        Ok(ResponseDeliverTx {
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
        _: RequestEndBlock,
    ) -> Result<ResponseEndBlock, eld_tendermint_state::Error> {
        Ok(ResponseEndBlock {
            validator_updates: Vec::new(),
            consensus_param_updates: None,
            events: Vec::new(),
        })
    }

    fn commit(&mut self) -> Result<ResponseCommit, eld_tendermint_state::Error> {
        Ok(ResponseCommit {
            data: Bytes::copy_from_slice(&[0xab; 32]),
            retain_height: 0,
        })
    }
}

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

fn block_id(byte: u8) -> BlockId {
    BlockId {
        hash: vec![byte; 32],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![byte.wrapping_add(1); 32],
        },
    }
}

fn home() -> std::path::PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-evidence-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn conflicting_votes(dir: &std::path::Path, chain_id: &str) -> (FilePV, Vote, Vote) {
    let key = dir.join("key.json");
    let state_a = dir.join("state_a.json");
    let state_b = dir.join("state_b.json");
    let state_node = dir.join("state_node.json");
    let pv = FilePV::load_or_gen_file_pv(&key, &state_a).expect("pv");
    std::fs::copy(&state_a, &state_b).expect("state b");
    std::fs::copy(&state_a, &state_node).expect("state node");
    let address = pv.get_pub_key().address().to_vec();
    let mut vote_a = Vote {
        vote_type: SignedMsgType::Prevote,
        height: 1,
        round: 0,
        block_id: block_id(1),
        timestamp: Time::from_unix_parts(1_600_000_000, 0),
        validator_address: address.clone(),
        validator_index: 0,
        signature: Vec::new(),
    };
    let mut vote_b = Vote {
        block_id: block_id(2),
        ..vote_a.clone()
    };
    let mut signer_a = FilePV::load(&key, &state_a).expect("signer a");
    let mut signer_b = FilePV::load(&key, &state_b).expect("signer b");
    signer_a.sign_vote(chain_id, &mut vote_a).expect("sign a");
    signer_b.sign_vote(chain_id, &mut vote_b).expect("sign b");
    let node_pv = FilePV::load(&key, &state_node).expect("node pv");
    (node_pv, vote_a, vote_b)
}

#[test]
fn duplicate_vote_is_proposed_once() {
    let dir = home();
    let chain_id = "test-chain";
    let (pv, vote_a, vote_b) = conflicting_votes(&dir, chain_id);
    let mut genesis = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new(chain_id),
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
    let evidence = DuplicateVoteEvidence::new(
        vote_a,
        vote_b,
        Time::from_unix_parts(1_600_000_000, 0),
        &state.validators,
    )
    .expect("evidence");
    let db = Arc::new(MemDb::new());
    let pool = Pool::new(Arc::clone(&db), &state);
    pool.add(evidence.clone()).expect("add");
    pool.add(evidence.clone()).expect("duplicate");
    let max_bytes = state.consensus_params.evidence.max_bytes;
    assert_eq!(pool.pending(max_bytes).len(), 1);
    assert!(pool.pending(0).is_empty());

    let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    let node = Node::start_with_evidence(
        ConsensusConfig::test_config(),
        pv,
        state.clone(),
        mempool,
        Exec,
        pool.clone().for_proposal(),
    )
    .expect("node");
    let mut group = Group::new(vec![node]);
    group.run_until_height(1);

    let block = group
        .nodes()
        .first_mut()
        .expect("node")
        .committed_block(1)
        .expect("height 1");
    let list = EvidenceList::new(vec![evidence.clone()]);
    assert_eq!(block.evidence.evidence.len(), 1);
    assert_eq!(block.header.evidence_hash, list.hash().as_bytes().to_vec());
    assert_eq!(block.header.evidence_hash, block.evidence.hash().as_bytes());

    let next = group
        .nodes()
        .first_mut()
        .expect("node")
        .proposal_block()
        .expect("height 2 proposal");
    assert_eq!(next.header.height, 2);
    assert!(next.evidence.evidence.is_empty());
    assert_eq!(
        next.header.evidence_hash,
        EvidenceList::default().hash().as_bytes()
    );

    let reopened = Pool::new(db, &state);
    assert!(reopened.pending(max_bytes).is_empty());
    reopened.add(evidence).expect("already committed");
    assert!(reopened.pending(max_bytes).is_empty());
}
