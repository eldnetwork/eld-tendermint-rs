//! WAL round-trip and catchup. No sleeps.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_privval::{Error as PvError, FilePV};
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseEndBlock,
};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::{App as ExecApp, State as ChainState, make_genesis_state};
use eld_tendermint_types::{
    BlockId, ChainId, GenesisDoc, GenesisValidator, PartSetHeader, Proposal, Time, Vote,
};
use prost::bytes::Bytes;

use eld_tendermint_consensus::{
    Error, Node, Wal, end_height_message, proposal_message, vote_message,
};

const APP_HASH: [u8; 32] = [0xab; 32];
const WHEN: Time = Time::from_unix_parts(1_600_000_000, 0);

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

struct Home {
    key: std::path::PathBuf,
    state: std::path::PathBuf,
    wal: std::path::PathBuf,
    chain: ChainState,
}

fn home() -> Home {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-wal-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pv = FilePV::load_or_gen_file_pv(dir.join("key.json"), dir.join("state.json")).expect("pv");
    let mut genesis = GenesisDoc {
        genesis_time: WHEN,
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
    let chain = make_genesis_state(&mut genesis).expect("genesis");
    Home {
        key: dir.join("key.json"),
        state: dir.join("state.json"),
        wal: dir.join("cs.wal").join("wal"),
        chain,
    }
}

fn load_pv(home: &Home) -> FilePV {
    FilePV::load_or_gen_file_pv(&home.key, &home.state).expect("pv")
}

fn start(home: &Home) -> Node<Exec, Check> {
    let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    Node::start_with_wal(
        ConsensusConfig::test_config(),
        load_pv(home),
        home.chain.clone(),
        mempool,
        Exec,
        &home.wal,
    )
    .expect("node")
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

#[test]
fn proposal_and_vote_round_trip() {
    let dir = std::env::temp_dir().join(format!("eld-wal-rt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wal");
    let mut pv =
        FilePV::load_or_gen_file_pv(dir.join("key.json"), dir.join("state.json")).expect("pv");
    let mut proposal = Proposal::new(1, 0, -1, block_id(1), WHEN);
    pv.sign_proposal("test-chain", &mut proposal)
        .expect("sign proposal");
    let mut vote = Vote {
        vote_type: SignedMsgType::Prevote,
        height: 1,
        round: 0,
        block_id: block_id(2),
        timestamp: WHEN,
        validator_address: pv.get_pub_key().address().to_vec(),
        validator_index: 0,
        signature: Vec::new(),
    };
    pv.sign_vote("test-chain", &mut vote).expect("sign vote");
    let written = vec![proposal_message(WHEN, &proposal), vote_message(WHEN, &vote)];
    {
        let mut wal = Wal::open(&path).expect("wal");
        for msg in &written {
            wal.write_message(msg).expect("write");
        }
    }
    let wal = Wal::open(&path).expect("reopen");
    let got = wal.read_messages().expect("read");
    let rest = &got[1..];
    if rest != written.as_slice() {
        panic!("wal records\n{rest:?}\n!=\n{written:?}");
    }
}

#[test]
fn reload_replays_a_durable_prevote_without_signing_again() {
    let home = home();
    let node = start(&home);
    let stored = node.last_signature().expect("prevote signature");
    drop(node);
    let node = start(&home);
    let again = node.last_signature().expect("stored signature");
    if again != stored {
        panic!("signature\n{again:?}\n!=\n{stored:?}");
    }
    assert_eq!(node.prevote_count(0), 1);
}

#[test]
fn conflicting_vote_during_replay_is_double_sign() {
    let home = home();
    let address = load_pv(&home).get_pub_key().address().to_vec();
    let node = start(&home);
    let stored = node.last_signature().expect("prevote signature");
    drop(node);
    {
        let mut wal = Wal::open(&home.wal).expect("wal");
        let vote = Vote {
            vote_type: SignedMsgType::Prevote,
            height: 1,
            round: 0,
            block_id: block_id(9),
            timestamp: WHEN,
            validator_address: address,
            validator_index: 0,
            signature: vec![0; 64],
        };
        wal.write_message(&vote_message(WHEN, &vote))
            .expect("conflict");
    }
    let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
    let started = Node::start_with_wal(
        ConsensusConfig::test_config(),
        load_pv(&home),
        home.chain.clone(),
        mempool,
        Exec,
        &home.wal,
    );
    let err = match started {
        Err(err) => err,
        Ok(_) => panic!("replay accepted a conflicting vote"),
    };
    assert!(
        matches!(err, Error::Privval(PvError::ConflictingData)),
        "{err}"
    );
    let again = load_pv(&home).last_sign_state.signature.expect("stored");
    if again != stored {
        panic!("signature\n{again:?}\n!=\n{stored:?}");
    }
}

#[test]
fn end_height_skips_the_finished_height() {
    let dir = std::env::temp_dir().join(format!("eld-wal-end-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("wal");
    let early = vote_message(
        WHEN,
        &Vote {
            vote_type: SignedMsgType::Prevote,
            height: 1,
            round: 0,
            block_id: block_id(1),
            timestamp: WHEN,
            validator_address: vec![1; 20],
            validator_index: 0,
            signature: vec![2; 64],
        },
    );
    let later = vote_message(
        WHEN,
        &Vote {
            vote_type: SignedMsgType::Prevote,
            height: 2,
            round: 0,
            block_id: block_id(3),
            timestamp: WHEN,
            validator_address: vec![1; 20],
            validator_index: 0,
            signature: vec![4; 64],
        },
    );
    {
        let mut wal = Wal::open(&path).expect("wal");
        wal.write_message(&early).expect("early");
        wal.write_message(&end_height_message(WHEN, 1))
            .expect("end");
        wal.write_message(&later).expect("later");
    }
    let wal = Wal::open(&path).expect("reopen");
    let after = wal
        .messages_after_end_height(1)
        .expect("scan")
        .expect("marker");
    if after.as_slice() != std::slice::from_ref(&later) {
        panic!("after end height\n{after:?}\n!=\n{later:?}");
    }
}
