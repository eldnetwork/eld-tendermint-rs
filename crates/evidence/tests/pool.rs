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
use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};
use eld_tendermint_state::{App as ExecApp, make_genesis_state};
use eld_tendermint_store::MemDb;
use eld_tendermint_types::{
    BlockId, ChainId, Commit, CommitSig, ConsensusVersion, DuplicateVoteEvidence, EvidenceList,
    GenesisDoc, GenesisValidator, Header, LightBlock, LightClientAttackEvidence, PartSetHeader,
    SignedHeader, Time, Validator, ValidatorSet, Vote,
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
    let list = EvidenceList::new(vec![evidence.clone().into()]);
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

#[test]
fn light_client_attack_is_proposed_once() {
    let dir = home();
    let chain_id = "test-chain";
    let (evidence, trusted, vals) = equivocation(&dir, chain_id);
    evidence
        .verify(&trusted, &trusted, &vals)
        .expect("equivocation verifies");
    let list = EvidenceList::new(vec![evidence.clone().into()]);
    assert_ne!(
        list.hash().as_bytes(),
        EvidenceList::default().hash().as_bytes()
    );

    let node_pv = FilePV::load_or_gen_file_pv(dir.join("node.json"), dir.join("node-state.json"))
        .expect("node pv");
    let mut genesis = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new(chain_id),
        initial_height: 0,
        consensus_params: None,
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key: node_pv.get_pub_key(),
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    let state = make_genesis_state(&mut genesis).expect("genesis");
    let db = Arc::new(MemDb::new());
    let pool = Pool::new(Arc::clone(&db), &state);
    pool.add_light(evidence.clone(), &trusted, &trusted, &vals)
        .expect("add");
    let max_bytes = state.consensus_params.evidence.max_bytes;
    assert_eq!(pool.pending(max_bytes).len(), 1);

    let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    let node = Node::start_with_evidence(
        ConsensusConfig::test_config(),
        node_pv,
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
}

fn equivocation(
    dir: &std::path::Path,
    chain_id: &str,
) -> (LightClientAttackEvidence, SignedHeader, ValidatorSet) {
    let when = Time::from_unix_parts(1_600_000_000, 0);
    let mut signers = Vec::new();
    for index in 0..5 {
        let pv = FilePV::generate(
            dir.join(format!("val-{index}.json")),
            dir.join(format!("val-{index}-state.json")),
        );
        pv.save().expect("save pv");
        signers.push(pv);
    }
    signers.sort_by(|left, right| {
        left.get_pub_key()
            .address()
            .cmp(&right.get_pub_key().address())
    });
    let validators = signers
        .iter()
        .map(|pv| Validator::new(pv.get_pub_key(), 10))
        .collect();
    let vals = ValidatorSet::new(validators).expect("validator set");
    let validators_hash = vals.hash().expect("set hash").as_bytes().to_vec();
    let proposer = vals.validators()[0].address.clone();
    let conflicting_header = attack_header(chain_id, when, &validators_hash, &proposer);
    let trusted_header = attack_header(
        chain_id,
        Time::from_unix_parts(1_600_000_100, 0),
        &validators_hash,
        &proposer,
    );
    let conflicting_id = commit_block_id(&conflicting_header);
    let trusted_id = commit_block_id(&trusted_header);
    let conflicting_commit = signed_commit(&signers, &vals, conflicting_id, chain_id, when, 4);
    let trusted_commit = signed_commit(&signers, &vals, trusted_id, chain_id, when, 5);
    let conflicting = SignedHeader {
        header: conflicting_header,
        commit: conflicting_commit,
    };
    let trusted = SignedHeader {
        header: trusted_header,
        commit: trusted_commit,
    };
    let mut evidence = LightClientAttackEvidence {
        conflicting_block: LightBlock {
            signed_header: conflicting,
            validator_set: vals.copy(),
        },
        common_height: 10,
        byzantine_validators: Vec::new(),
        total_voting_power: vals.total_voting_power(),
        timestamp: when,
    };
    evidence.byzantine_validators = evidence.get_byzantine_validators(&vals, &trusted);
    (evidence, trusted, vals)
}

fn attack_header(chain_id: &str, time: Time, validators_hash: &[u8], proposer: &[u8]) -> Header {
    Header {
        version: ConsensusVersion::default(),
        chain_id: ChainId::new(chain_id),
        height: 10,
        time,
        last_block_id: BlockId::default(),
        last_commit_hash: vec![0x22; 32],
        data_hash: vec![0x33; 32],
        validators_hash: validators_hash.to_vec(),
        next_validators_hash: vec![0x44; 32],
        consensus_hash: vec![0x55; 32],
        app_hash: vec![0x66; 32],
        last_results_hash: vec![0x77; 32],
        evidence_hash: vec![0x88; 32],
        proposer_address: proposer.to_vec(),
    }
}

fn commit_block_id(header: &Header) -> BlockId {
    let mut parts = [0_u8; 32];
    parts[..9].copy_from_slice(b"partshash");
    BlockId {
        hash: header.hash().expect("header hash").as_bytes().to_vec(),
        part_set_header: PartSetHeader {
            total: 1000,
            hash: parts.to_vec(),
        },
    }
}

fn signed_commit(
    signers: &[FilePV],
    vals: &ValidatorSet,
    block_id: BlockId,
    chain_id: &str,
    time: Time,
    signed: usize,
) -> Commit {
    let mut signatures = Vec::with_capacity(vals.validators().len());
    for (index, validator) in vals.validators().iter().enumerate() {
        if index >= signed {
            signatures.push(CommitSig::absent());
            continue;
        }
        let pv = signers
            .iter()
            .find(|pv| pv.get_pub_key().address().as_slice() == validator.address.as_slice())
            .expect("signer");
        let mut vote = Vote {
            vote_type: SignedMsgType::Precommit,
            height: 10,
            round: 1,
            block_id: block_id.clone(),
            timestamp: time,
            validator_address: validator.address.clone(),
            validator_index: i32::try_from(index).unwrap_or(i32::MAX),
            signature: Vec::new(),
        };
        pv.clone().sign_vote(chain_id, &mut vote).expect("sign");
        signatures.push(CommitSig {
            block_id_flag: BlockIdFlag::Commit,
            validator_address: validator.address.clone(),
            timestamp: vote.timestamp,
            signature: vote.signature,
        });
    }
    Commit {
        height: 10,
        round: 1,
        block_id,
        signatures,
    }
}
