//! Two switches gossiping consensus. No catchup.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_consensus::{Node, Reactor, Step, channel_descriptors};
use eld_tendermint_crypto::PrivKey;
use eld_tendermint_mempool::{App as MempoolApp, Mempool};
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock,
    ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseEndBlock,
};
use eld_tendermint_proto::consensus::{self, BlockPart, Message, message};
use eld_tendermint_state::{App as ExecApp, make_genesis_state};
use eld_tendermint_types::{ChainId, GenesisDoc, GenesisValidator, Part, Time};
use prost::Message as ProstMessage;
use prost::bytes::Bytes;

use eld_tendermint_consensus::DATA_CHANNEL;

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

fn nodes(n: usize) -> Vec<Node<Exec, Check>> {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-reactor-{}-{}-{}",
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
    keys.into_iter()
        .map(|pv| {
            let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
            Node::start(config.clone(), pv, state.clone(), mempool, Exec).expect("node")
        })
        .collect()
}

fn secret_pair() -> (
    eld_tendermint_p2p::SecretConnection<UnixStream>,
    eld_tendermint_p2p::SecretConnection<UnixStream>,
) {
    let key_a = PrivKey::generate();
    let key_b = PrivKey::generate();
    let (sock_a, sock_b) = UnixStream::pair().expect("socket pair");
    sock_a
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    sock_b
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let handle = thread::spawn(move || make_secret_connection(sock_b, &key_b).expect("handshake"));
    let conn_a = make_secret_connection(sock_a, &key_a).expect("handshake");
    let conn_b = handle.join().expect("peer handshake");
    (conn_a, conn_b)
}

fn switch_for(reactor: &Reactor<Exec, Check>) -> Arc<Switch> {
    let switch = Arc::new(Switch::new());
    let callback_switch = Arc::clone(&switch);
    let callback_reactor = reactor.clone();
    switch
        .add_reactor(
            "consensus",
            channel_descriptors(),
            move |peer_id, ch_id, bytes| {
                if !callback_reactor.handle(peer_id, ch_id, &bytes) {
                    callback_switch.stop_peer(peer_id);
                }
            },
        )
        .expect("reactor");
    switch
}

fn link(left: &Switch, right: &Switch) {
    let (conn_left, conn_right) = secret_pair();
    left.add_peer(conn_left, "right").expect("peer");
    right.add_peer(conn_right, "left").expect("peer");
}

fn encode_proposal(proposal: &eld_tendermint_types::Proposal) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::Proposal(consensus::Proposal {
            proposal: Some(proposal.to_proto()),
        })),
    }
    .encode_to_vec()
}

#[test]
fn two_switches_commit_height_one() {
    let mut validators = nodes(4);
    let right_nodes = validators.split_off(2);
    let left = Reactor::new(validators);
    let right = Reactor::new(right_nodes);
    let left_switch = switch_for(&left);
    let right_switch = switch_for(&right);
    link(&left_switch, &right_switch);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if left.committed(1) && right.committed(1) {
            return;
        }
        left.poll(&left_switch);
        right.poll(&right_switch);
        thread::sleep(Duration::from_millis(2));
    }
    panic!(
        "height left={} right={}",
        left.committed(1),
        right.committed(1)
    );
}

#[test]
fn bad_proposal_signature_does_not_prevote_or_stop() {
    let mut validators = nodes(4);
    let proposer = validators[0].proposer_after(0).expect("proposer");
    let proposer_index = validators
        .iter()
        .position(|node| node.address() == proposer)
        .expect("proposer index");
    let victim_index = (proposer_index + 1) % validators.len();
    let proposal = validators[proposer_index]
        .queued_proposal()
        .expect("signed proposal");
    let victim = validators.swap_remove(victim_index);
    let reactor = Reactor::new(vec![victim]);
    let receiver = switch_for(&reactor);
    let sender = Switch::new();
    sender
        .add_reactor("consensus", channel_descriptors(), |_, _, _| {})
        .expect("sender reactor");
    link(&receiver, &sender);

    let mut bad = proposal;
    bad.signature = vec![0xab; 64];
    assert!(sender.send("left", DATA_CHANNEL, &encode_proposal(&bad)));

    thread::sleep(Duration::from_millis(30));
    assert_eq!(reactor.step(), Step::Propose);
    assert_eq!(reactor.prevote_count(0), 0);
    assert_eq!(receiver.peers().len(), 1);
}

#[test]
fn bad_part_proof_is_dropped_and_peer_stays() {
    let mut validators = nodes(4);
    let proposer = validators[0].proposer_after(0).expect("proposer");
    let proposer_index = validators
        .iter()
        .position(|node| node.address() == proposer)
        .expect("proposer index");
    let proposal = validators[proposer_index]
        .queued_proposal()
        .expect("signed proposal");
    let victim_index = (proposer_index + 1) % validators.len();
    let victim = validators.swap_remove(victim_index);
    let reactor = Reactor::new(vec![victim]);
    let receiver = switch_for(&reactor);
    let sender = Switch::new();
    sender
        .add_reactor("consensus", channel_descriptors(), |_, _, _| {})
        .expect("sender reactor");
    link(&receiver, &sender);

    assert!(sender.send("left", DATA_CHANNEL, &encode_proposal(&proposal)));
    let start = Instant::now();
    while reactor.proposal_hash().is_none() && start.elapsed() < Duration::from_millis(200) {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        reactor.proposal_hash().is_some(),
        "proposal was not accepted"
    );

    let part = Part {
        index: 0,
        bytes: b"not-the-block".to_vec(),
        proof: eld_tendermint_crypto::Proof {
            total: 1,
            index: 0,
            leaf_hash: vec![0x11; 32],
            aunts: Vec::new(),
        },
    };
    let bytes = Message {
        sum: Some(message::Sum::BlockPart(BlockPart {
            height: 1,
            round: 0,
            part: Some(part.to_proto()),
        })),
    }
    .encode_to_vec();
    assert!(sender.send("left", DATA_CHANNEL, &bytes));
    thread::sleep(Duration::from_millis(30));

    assert_eq!(reactor.step(), Step::Propose);
    assert_eq!(reactor.prevote_count(0), 0);
    assert!(reactor.proposal_hash().is_some());
    assert_eq!(receiver.peers().len(), 1);
}
