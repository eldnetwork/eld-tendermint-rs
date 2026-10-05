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
    assert!(validators.nodes()[receiver].on_proposal(proposal));
    assert_eq!(validators.nodes()[receiver].prevote_count(0), 0);
    assert_eq!(validators.nodes()[receiver].step(), Step::Propose);
}

#[test]
fn bad_pol_round_is_rejected_and_a_repeat_is_kept() {
    let mut validators = group(4);
    let proposer = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_some())
        .expect("proposer");
    let receiver = (proposer + 1) % 4;
    let again = (proposer + 2) % 4;
    let proposal = validators.nodes()[proposer]
        .queued_proposal()
        .expect("proposal");
    assert!(validators.nodes()[receiver].on_proposal(proposal.clone()));
    assert!(
        validators.nodes()[receiver].on_proposal(proposal.clone()),
        "a second copy of the same signature dropped the peer"
    );
    assert_eq!(validators.nodes()[receiver].step(), Step::Propose);

    let mut same_round = proposal.clone();
    same_round.pol_round = same_round.round;
    assert!(
        !validators.nodes()[again].on_proposal(same_round),
        "POLRound == round was accepted"
    );
    let mut negative = proposal;
    negative.pol_round = -2;
    assert!(
        !validators.nodes()[again].on_proposal(negative),
        "POLRound -2 was accepted"
    );
    assert_eq!(validators.nodes()[again].step(), Step::Propose);
    assert_eq!(validators.nodes()[again].prevote_count(0), 0);
}

/// Round-1 re-proposal on `locked`, and `fresh` already in round 1 without a round-0 majority.
struct Round1Setup {
    validators: Group<Exec, Check>,
    locked: usize,
    fresh: usize,
    donor: usize,
    block_hash: Vec<u8>,
}

fn give_block(validators: &mut Group<Exec, Check>, from: usize, to: usize) {
    let proposal = validators.nodes()[from]
        .queued_proposal()
        .expect("proposal");
    let parts = validators.nodes()[from].queued_parts();
    assert!(!parts.is_empty(), "proposal has no parts");
    assert!(validators.nodes()[to].on_proposal(proposal));
    for part in parts {
        assert!(validators.nodes()[to].on_part(part));
    }
    assert!(
        validators.nodes()[to]
            .queued_vote(SignedMsgType::Prevote)
            .is_some(),
        "validator {to} did not prevote the block"
    );
}

fn round1_setup() -> Round1Setup {
    let mut validators = group(4);
    let proposer = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_some())
        .expect("round 0 proposer");
    let next = validators.nodes()[proposer]
        .proposer_after(1)
        .expect("round 1 proposer");
    let locked = validators
        .nodes()
        .iter()
        .position(|node| node.address() == next)
        .expect("round 1 proposer index");
    assert_ne!(locked, proposer);
    let mut rest = (0..4).filter(|index| *index != proposer && *index != locked);
    let fresh = rest.next().expect("fresh");
    let donor = rest.next().expect("donor");
    let block_hash = validators.nodes()[proposer]
        .queued_proposal()
        .expect("proposal")
        .block_id
        .hash
        .clone();

    give_block(&mut validators, proposer, locked);
    give_block(&mut validators, proposer, donor);

    assert_eq!(validators.nodes()[fresh].step(), Step::Propose);
    validators.fire_timeout(fresh);
    let nil = validators.nodes()[fresh]
        .queued_vote(SignedMsgType::Prevote)
        .expect("nil prevote");
    assert!(nil.block_id.hash.is_empty(), "fresh prevoted a block");
    let proposer_prevote = validators.nodes()[proposer]
        .queued_vote(SignedMsgType::Prevote)
        .expect("proposer prevote");
    let locked_prevote = validators.nodes()[locked]
        .queued_vote(SignedMsgType::Prevote)
        .expect("locked prevote");
    validators.nodes()[fresh].on_vote(nil);
    validators.nodes()[fresh].on_vote(proposer_prevote);
    validators.nodes()[fresh].on_vote(locked_prevote);
    assert_eq!(validators.nodes()[fresh].step(), Step::PrevoteWait);
    validators.fire_timeout(fresh);
    assert_eq!(validators.nodes()[fresh].step(), Step::Precommit);

    let own_precommit = validators.nodes()[fresh]
        .queued_vote(SignedMsgType::Precommit)
        .expect("nil precommit");
    let proposer_nil = validators.nodes()[proposer]
        .sign_nil_precommit()
        .expect("proposer nil precommit");
    let donor_nil = validators.nodes()[donor]
        .sign_nil_precommit()
        .expect("donor nil precommit");
    validators.nodes()[fresh].on_vote(own_precommit);
    validators.nodes()[fresh].on_vote(proposer_nil.clone());
    validators.nodes()[fresh].on_vote(donor_nil.clone());
    assert_eq!(validators.nodes()[fresh].step(), Step::PrecommitWait);
    validators.fire_timeout(fresh);
    assert_eq!(validators.nodes()[fresh].round(), 1);
    assert_eq!(validators.nodes()[fresh].step(), Step::Propose);

    let proposer_prevote = validators.nodes()[proposer]
        .queued_vote(SignedMsgType::Prevote)
        .expect("proposer prevote");
    let donor_prevote = validators.nodes()[donor]
        .queued_vote(SignedMsgType::Prevote)
        .expect("donor prevote");
    let locked_prevote = validators.nodes()[locked]
        .queued_vote(SignedMsgType::Prevote)
        .expect("own prevote");
    validators.nodes()[locked].on_vote(proposer_prevote);
    validators.nodes()[locked].on_vote(donor_prevote);
    validators.nodes()[locked].on_vote(locked_prevote);
    assert_eq!(validators.nodes()[locked].step(), Step::Precommit);
    let locked_precommit = validators.nodes()[locked]
        .queued_vote(SignedMsgType::Precommit)
        .expect("block precommit");
    let proposer_nil = validators.nodes()[proposer]
        .sign_nil_precommit()
        .expect("proposer nil precommit");
    let donor_nil = validators.nodes()[donor]
        .sign_nil_precommit()
        .expect("donor nil precommit");
    validators.nodes()[locked].on_vote(locked_precommit);
    validators.nodes()[locked].on_vote(proposer_nil);
    validators.nodes()[locked].on_vote(donor_nil);
    assert_eq!(validators.nodes()[locked].step(), Step::PrecommitWait);
    validators.fire_timeout(locked);
    assert_eq!(validators.nodes()[locked].round(), 1);
    let proposal = validators.nodes()[locked]
        .queued_proposal()
        .expect("round 1 proposal");
    assert!(proposal.pol_round >= 0, "pol_round {}", proposal.pol_round);
    assert_eq!(proposal.block_id.hash, block_hash);

    Round1Setup {
        validators,
        locked,
        fresh,
        donor,
        block_hash,
    }
}

fn deliver_round1_proposal(setup: &mut Round1Setup) {
    let proposal = setup.validators.nodes()[setup.locked]
        .queued_proposal()
        .expect("round 1 proposal");
    let parts = setup.validators.nodes()[setup.locked].queued_parts();
    assert!(!parts.is_empty(), "round 1 proposal has no parts");
    assert!(setup.validators.nodes()[setup.fresh].on_proposal(proposal));
    for part in parts {
        assert!(setup.validators.nodes()[setup.fresh].on_part(part));
    }
}

#[test]
fn re_proposal_without_pol_prevotes_nil() {
    let mut setup = round1_setup();
    deliver_round1_proposal(&mut setup);
    assert_eq!(setup.validators.nodes()[setup.fresh].step(), Step::Propose);
    setup.validators.fire_timeout(setup.fresh);
    let vote = setup.validators.nodes()[setup.fresh]
        .queued_vote(SignedMsgType::Prevote)
        .expect("round 1 prevote");
    assert!(
        vote.block_id.hash.is_empty(),
        "prevoted the block without a POL majority"
    );
    assert_eq!(vote.round, 1);
}

#[test]
fn re_proposal_with_pol_prevotes_the_block() {
    let mut setup = round1_setup();
    let pol = setup.validators.nodes()[setup.donor]
        .queued_vote(SignedMsgType::Prevote)
        .expect("donor block prevote");
    setup.validators.nodes()[setup.fresh].on_vote(pol);
    deliver_round1_proposal(&mut setup);
    let vote = setup.validators.nodes()[setup.fresh]
        .queued_vote(SignedMsgType::Prevote)
        .expect("round 1 prevote");
    assert_eq!(vote.block_id.hash, setup.block_hash);
    assert_eq!(vote.round, 1);
}

#[test]
fn locked_validator_prevotes_the_locked_block() {
    let mut setup = round1_setup();
    let vote = setup.validators.nodes()[setup.locked]
        .queued_vote(SignedMsgType::Prevote)
        .expect("round 1 prevote");
    assert_eq!(vote.round, 1);
    assert_eq!(vote.block_id.hash, setup.block_hash);
}
