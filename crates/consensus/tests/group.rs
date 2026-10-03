//! In-process rounds. No sockets and no sleeps.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::{Error as PvError, FilePV};
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseEndBlock,
};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::{App as ExecApp, make_genesis_state};
use eld_tendermint_types::{
    BlockId, ChainId, GenesisDoc, GenesisValidator, Time, Vote, set_log_capture, upper_hex,
};
use prost::bytes::Bytes;

use eld_tendermint_consensus::{Group, Msg, Node, Step};

const APP_HASH: [u8; 32] = [0xab; 32];

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
            data: Bytes::copy_from_slice(&APP_HASH),
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

fn group(n: usize) -> Group<Exec, Check> {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-consensus-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut keys = Vec::with_capacity(n);
    for index in 0..n {
        let pv = FilePV::load_or_gen_file_pv(
            dir.join(format!("priv_validator_key_{index}.json")),
            dir.join(format!("priv_validator_state_{index}.json")),
        )
        .expect("file pv");
        keys.push(pv);
    }
    let mut genesis = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: None,
        validators: keys
            .iter()
            .map(|pv| GenesisValidator {
                address: Vec::new(),
                pub_key: pv.get_pub_key(),
                power: 10,
                name: String::new(),
            })
            .collect(),
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    let state = make_genesis_state(&mut genesis).expect("genesis");
    let config = ConsensusConfig::test_config();
    let nodes = keys
        .into_iter()
        .map(|pv| {
            let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
            Node::start(config.clone(), pv, state.clone(), mempool, Exec).expect("node")
        })
        .collect();
    Group::new(nodes)
}

fn is_block_data(msg: &Msg) -> bool {
    matches!(msg, Msg::Proposal(_) | Msg::Part(_))
}

fn is_prevote(msg: &Msg) -> bool {
    matches!(msg, Msg::Vote(vote) if vote.vote_type == SignedMsgType::Prevote)
}

#[test]
fn one_validator_commits_height_one() {
    let capture = LogCapture::start();
    let mut validators = group(1);
    validators.run_until_height(1);
    assert_committed(&mut validators);
    let logs = capture.text();
    let hash = upper_hex(&validators.nodes()[0].committed_hash(1).expect("block"));
    assert!(
        logs.lines()
            .any(|line| line.contains("Committed block") && line.contains(&hash)),
        "{logs}"
    );
    assert!(
        logs.lines().any(|line| {
            line.contains("received complete proposal block") && line.contains(&hash)
        }),
        "{logs}"
    );
}

/// Records operator lines for one test, then stops recording.
struct LogCapture(Arc<Mutex<String>>);

impl LogCapture {
    fn start() -> Self {
        let buf = Arc::new(Mutex::new(String::new()));
        set_log_capture(Some(Arc::clone(&buf)));
        Self(buf)
    }

    fn text(&self) -> String {
        self.0.lock().unwrap_or_else(|err| err.into_inner()).clone()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        set_log_capture(None);
    }
}

#[test]
fn four_validators_commit_height_one() {
    let mut validators = group(4);
    validators.run_until_height(1);
    assert_committed(&mut validators);
}

fn assert_committed(validators: &mut Group<Exec, Check>) {
    let nodes = validators.nodes();
    assert!(nodes.iter().all(|node| node.store_height() == 1));
    let hashes: Vec<_> = nodes
        .iter()
        .map(|node| node.committed_hash(1).expect("block"))
        .collect();
    for hash in &hashes {
        if hash != &hashes[0] {
            panic!("committed hashes\n{hashes:?}");
        }
    }
    assert_eq!(hashes[0].len(), 32);
}

#[test]
fn locked_validator_proposes_the_same_block_in_round_one() {
    let mut validators = group(4);
    let round0_index = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_some())
        .expect("round 0 proposer");
    let round0 = validators.nodes()[round0_index]
        .queued_proposal()
        .expect("round 0 proposal")
        .block_id
        .hash;
    let next = validators.nodes()[round0_index]
        .proposer_after(1)
        .expect("round 1 proposer");
    let index = validators
        .nodes()
        .iter()
        .position(|node| node.address() == next)
        .expect("proposer index");
    assert_ne!(
        validators.nodes()[index].address(),
        validators.nodes()[round0_index]
            .proposer_after(0)
            .expect("round 0")
    );
    validators.exchange(None, is_block_data);
    validators.exchange(Some(index), is_prevote);
    assert_eq!(validators.nodes()[index].reap_count(), 0);
    for other in 0..4 {
        if other == index {
            continue;
        }
        let vote = validators.nodes()[other]
            .sign_nil_precommit()
            .expect("nil precommit");
        validators.nodes()[index].on_vote(vote);
    }
    validators.fire_timeout(index);
    let proposed = validators.nodes()[index]
        .proposal_hash()
        .expect("round 1 proposal");
    if proposed != round0 {
        panic!("round 1 proposal\n{proposed:?}\n!= locked block\n{round0:?}");
    }
    assert_eq!(validators.nodes()[index].reap_count(), 0);
}

#[test]
fn file_pv_rejects_a_different_vote_at_the_same_step() {
    let dir = std::env::temp_dir().join(format!("eld-pv-conflict-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut pv =
        FilePV::load_or_gen_file_pv(dir.join("key.json"), dir.join("state.json")).expect("pv");
    let mut vote = Vote {
        vote_type: SignedMsgType::Prevote,
        height: 1,
        round: 0,
        block_id: BlockId {
            hash: vec![1; 32],
            part_set_header: eld_tendermint_types::PartSetHeader {
                total: 1,
                hash: vec![2; 32],
            },
        },
        timestamp: Time::from_unix_parts(1_600_000_000, 0),
        validator_address: pv.get_pub_key().address().to_vec(),
        validator_index: 0,
        signature: Vec::new(),
    };
    pv.sign_vote("test-chain", &mut vote).expect("first sign");
    let stored = pv
        .last_sign_state
        .signature
        .clone()
        .expect("stored signature");
    vote.block_id.hash = vec![3; 32];
    let err = pv.sign_vote("test-chain", &mut vote).expect_err("conflict");
    assert!(matches!(err, PvError::ConflictingData), "{err}");
    assert_eq!(pv.last_sign_state.signature.as_ref(), Some(&stored));
}

#[test]
fn bad_proposal_signature_is_not_prevoted() {
    let mut validators = group(4);
    let proposer = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_some())
        .expect("proposer");
    let receiver = (proposer + 1) % 4;
    let mut proposal = validators.nodes()[proposer]
        .queued_proposal()
        .expect("proposal");
    proposal.signature[0] ^= 0xff;
    validators.nodes()[receiver].on_proposal(proposal);
    assert_eq!(validators.nodes()[receiver].prevote_count(0), 0);
    assert_eq!(validators.nodes()[receiver].step(), Step::Propose);
}
