//! Two switches gossiping consensus. A node one or two blocks behind catches up
//! from block-store parts and seen-commit votes, with the blockchain reactor off.

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
use eld_tendermint_state::{App as ExecApp, State as ChainState, make_genesis_state};
use eld_tendermint_store::{BlockStore, MemDb};
use eld_tendermint_types::{ChainId, GenesisDoc, GenesisValidator, Part, Time, set_log_capture};
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
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    sock_b
        .set_read_timeout(Some(Duration::from_secs(30)))
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
                if !callback_reactor.handle(&callback_switch, peer_id, ch_id, &bytes) {
                    callback_switch.stop_peer(peer_id);
                }
            },
        )
        .expect("reactor");
    switch
}

fn link(left: &Switch, right: &Switch) {
    link_ids(left, "left", right, "right");
}

fn link_ids(left: &Switch, left_id: &str, right: &Switch, right_id: &str) {
    let (conn_left, conn_right) = secret_pair();
    left.add_peer(conn_left, right_id).expect("peer");
    right.add_peer(conn_right, left_id).expect("peer");
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

/// The late node has no link to the proposer. The block body has to cross the relay.
#[test]
fn relay_forwards_proposal_parts_without_a_direct_link() {
    let mut validators = nodes(4);
    let proposer_address = validators[0].proposer_after(0).expect("proposer");
    let proposer_index = validators
        .iter()
        .position(|node| node.address() == proposer_address)
        .expect("proposer index");
    let proposer_node = validators.swap_remove(proposer_index);
    let late_node = validators.pop().expect("late");
    let relay_node = validators.pop().expect("relay");
    drop(validators);

    let proposer = Reactor::new(vec![proposer_node]);
    let relay = Reactor::new(vec![relay_node]);
    let late = Reactor::new(vec![late_node]);
    let proposer_switch = switch_for(&proposer);
    let relay_switch = switch_for(&relay);
    let late_switch = switch_for(&late);
    link_ids(&proposer_switch, "proposer", &relay_switch, "relay");
    link_ids(&relay_switch, "relay", &late_switch, "late");
    assert!(
        late_switch.peers().iter().all(|peer| peer.id != "proposer"),
        "late node is linked to the proposer"
    );

    let capture = LogCapture::start();
    let start = Instant::now();
    let committed = loop {
        if proposer.committed(1) && relay.committed(1) && late.committed(1) {
            break true;
        }
        if start.elapsed() >= Duration::from_secs(8) {
            break false;
        }
        proposer.poll(&proposer_switch);
        relay.poll(&relay_switch);
        late.poll(&late_switch);
        thread::sleep(Duration::from_millis(2));
    };
    assert!(
        committed,
        "height proposer={} relay={} late={}",
        proposer.committed(1),
        relay.committed(1),
        late.committed(1)
    );
    let logs = capture.text();
    assert!(
        logs.lines()
            .any(|line| line.contains("received complete proposal block")),
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

fn poll_until(reactors: &[(&Reactor<Exec, Check>, &Switch)], height: i64, limit: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if reactors
            .iter()
            .all(|(reactor, _)| reactor.committed(height))
        {
            return true;
        }
        for (reactor, switch) in reactors {
            reactor.poll(switch);
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

fn bad_catchup_part(height: i64) -> Vec<u8> {
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
    Message {
        sum: Some(message::Sum::BlockPart(BlockPart {
            height,
            round: 0,
            part: Some(part.to_proto()),
        })),
    }
    .encode_to_vec()
}

struct CatchupNodes {
    ahead: Vec<Node<Exec, Check>>,
    behind: Node<Exec, Check>,
    store: Arc<BlockStore<MemDb>>,
    state: ChainState,
    config: ConsensusConfig,
    wal_path: std::path::PathBuf,
    key_path: std::path::PathBuf,
    state_path: std::path::PathBuf,
}

/// A holds three of four equal-power validators. B holds the other and a file WAL.
fn catchup_validators() -> CatchupNodes {
    static N: AtomicU64 = AtomicU64::new(0);
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "eld-catchup-{}-{}-{}",
        std::process::id(),
        stamp,
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let mut keys = Vec::with_capacity(4);
    let mut paths = Vec::with_capacity(4);
    for index in 0..4 {
        let key_path = dir.join(format!("priv_validator_key_{index}.json"));
        let state_path = dir.join(format!("priv_validator_state_{index}.json"));
        let pv = FilePV::load_or_gen_file_pv(&key_path, &state_path).expect("file pv");
        paths.push((key_path, state_path));
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
    let wal_path = dir.join("cs.wal");
    let store = Arc::new(BlockStore::new(MemDb::new()));
    let (b_key, b_state) = paths.pop().expect("behind paths");
    let behind_key = keys.pop().expect("behind key");
    let behind = Node::start_with_wal_and_store(
        config.clone(),
        behind_key,
        state.clone(),
        Mempool::new(MempoolConfig::test_config(), Check).expect("mempool"),
        Exec,
        Arc::clone(&store),
        &wal_path,
    )
    .expect("behind node");
    let ahead = keys
        .into_iter()
        .map(|pv| {
            let mempool = Mempool::new(MempoolConfig::test_config(), Check).expect("mempool");
            Node::start(config.clone(), pv, state.clone(), mempool, Exec).expect("node")
        })
        .collect();
    CatchupNodes {
        ahead,
        behind,
        store,
        state,
        config,
        wal_path,
        key_path: b_key,
        state_path: b_state,
    }
}

#[test]
fn lagging_node_catches_up_from_height_one_to_three() {
    let CatchupNodes {
        ahead: ahead_nodes,
        behind: behind_node,
        store,
        state,
        config,
        wal_path,
        key_path,
        state_path,
    } = catchup_validators();
    let ahead = Reactor::new(ahead_nodes);
    let behind = Reactor::new(vec![behind_node]);
    let ahead_switch = switch_for(&ahead);
    let behind_switch = switch_for(&behind);
    link(&ahead_switch, &behind_switch);

    let start = Instant::now();
    while !ahead.committed(1) && start.elapsed() < Duration::from_secs(5) {
        ahead.poll(&ahead_switch);
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(ahead.committed(1), "ahead did not commit height 1");
    // B's height is in A's peer state. One more poll sends block 1 and only
    // arms the next round; a further poll would commit height 2.
    behind.poll(&behind_switch);
    thread::sleep(Duration::from_millis(30));
    let start = Instant::now();
    while !behind.committed(1) && start.elapsed() < Duration::from_secs(5) {
        ahead.gossip(&ahead_switch);
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        ahead.committed(1) && behind.committed(1),
        "height 1 ahead={} behind={}",
        ahead.committed(1),
        behind.committed(1)
    );
    behind.poll(&behind_switch);
    thread::sleep(Duration::from_millis(30));
    ahead_switch.stop_peer("right");
    behind_switch.stop_peer("left");
    assert!(
        !ahead.committed(2),
        "ahead left height 1 before the partition"
    );

    let start = Instant::now();
    while !has_prevote(&behind) && start.elapsed() < Duration::from_secs(2) {
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    let lagging_prevote = behind
        .last_signature()
        .expect("prevote at the lagging height");
    assert!(!lagging_prevote.is_empty());

    assert!(
        poll_until(&[(&ahead, &ahead_switch)], 3, Duration::from_secs(8)),
        "ahead did not reach store height 3"
    );
    assert!(!behind.committed(2), "behind advanced while partitioned");

    link(&ahead_switch, &behind_switch);
    assert!(ahead_switch.send("right", DATA_CHANNEL, &bad_catchup_part(2)));
    thread::sleep(Duration::from_millis(30));
    assert!(
        !behind.committed(2),
        "a bad part proof advanced the behind node"
    );
    assert_eq!(behind_switch.peers().len(), 1, "bad part stopped the peer");

    let start = Instant::now();
    while !behind.committed(2) && start.elapsed() < Duration::from_secs(8) {
        ahead.gossip(&ahead_switch);
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        behind.committed(2),
        "behind did not apply block 2 peers={}",
        behind_switch.peers().len()
    );
    for _ in 0..20 {
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    while !behind.committed(3) && start.elapsed() < Duration::from_secs(8) {
        ahead.gossip(&ahead_switch);
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        behind.committed(3),
        "behind did not catch up to store height 3"
    );
    assert_eq!(behind.committed_hash(3), ahead.committed_hash(3));
    assert!(behind.committed_hash(3).is_some());

    let start = Instant::now();
    while !has_prevote(&behind) && start.elapsed() < Duration::from_secs(2) {
        behind.poll(&behind_switch);
        thread::sleep(Duration::from_millis(2));
    }
    let before_restart = behind.last_signature().expect("signature on disk");
    drop(behind_switch);
    drop(behind);
    let restarted = Node::start_with_wal_and_store(
        config,
        FilePV::load_or_gen_file_pv(&key_path, &state_path).expect("reload pv"),
        state,
        Mempool::new(MempoolConfig::test_config(), Check).expect("mempool"),
        Exec,
        store,
        &wal_path,
    )
    .expect("wal replay");
    assert_eq!(restarted.last_signature(), Some(before_restart));
}

fn has_prevote(reactor: &Reactor<Exec, Check>) -> bool {
    (0..32).any(|round| reactor.prevote_count(round) > 0)
}
