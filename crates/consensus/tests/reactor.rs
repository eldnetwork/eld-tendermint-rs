//! Two switches gossiping consensus. A node one or two blocks behind catches up
//! from block-store parts and seen-commit votes, with the blockchain reactor off.

use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{ConsensusConfig, MempoolConfig};
use eld_tendermint_consensus::{
    DATA_CHANNEL, Group, Msg, Node, Reactor, STATE_CHANNEL, Step, VOTE_CHANNEL,
    VOTE_SET_BITS_CHANNEL, channel_descriptors,
};
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
use eld_tendermint_types::{
    BitArray, BlockId, ChainId, GenesisDoc, GenesisValidator, Part, PartSetHeader, Proposal,
    SignedMsgType, Time, set_log_capture,
};
use prost::Message as ProstMessage;
use prost::bytes::Bytes;

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

fn is_block_data(msg: &Msg) -> bool {
    matches!(msg, Msg::Proposal(_) | Msg::Part(_))
}

fn is_prevote(msg: &Msg) -> bool {
    matches!(msg, Msg::Vote(vote) if vote.vote_type == SignedMsgType::Prevote)
}

type Captured = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

fn capturing_switch() -> (Arc<Switch>, Captured) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let switch = Arc::new(Switch::new());
    let got_cb = Arc::clone(&got);
    switch
        .add_reactor(
            "consensus",
            channel_descriptors(),
            move |_peer_id, ch_id, bytes| {
                got_cb
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .push((ch_id, bytes));
            },
        )
        .expect("capture reactor");
    (switch, got)
}

fn encode_round_step(height: i64, round: i32, step: u32, last_commit_round: i32) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::NewRoundStep(consensus::NewRoundStep {
            height,
            round,
            step,
            seconds_since_start_time: 0,
            last_commit_round,
        })),
    }
    .encode_to_vec()
}

fn encode_proposal_pol(height: i64, pol_round: i32, bits: &BitArray) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::ProposalPol(consensus::ProposalPol {
            height,
            proposal_pol_round: pol_round,
            proposal_pol: bits.to_proto(),
        })),
    }
    .encode_to_vec()
}

fn bits_with(index: i64) -> BitArray {
    let mut bits = BitArray::new(4).expect("bits");
    bits.set_index(index, true);
    bits
}

/// A round-1 re-proposal carries `pol_round >= 0`, so the data channel also gets `ProposalPol`.
#[test]
fn re_proposal_sends_proposal_pol() {
    let mut validators = Group::new(nodes(4));
    let round0_index = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_some())
        .expect("round 0 proposer");
    let next = validators.nodes()[round0_index]
        .proposer_after(1)
        .expect("round 1 proposer");
    let index = validators
        .nodes()
        .iter()
        .position(|node| node.address() == next)
        .expect("proposer index");
    validators.exchange(None, is_block_data);
    validators.exchange(Some(index), is_prevote);
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
    let proposal = validators.nodes()[index]
        .queued_proposal()
        .expect("round 1 proposal");
    assert!(proposal.pol_round >= 0, "pol_round {}", proposal.pol_round);
    let height = proposal.height;
    let round = proposal.round;
    let pol_round = proposal.pol_round;

    let reactor = Reactor::new(validators.into_nodes());
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    assert!(reactor.handle(
        &reactor_switch,
        "right",
        STATE_CHANNEL,
        &encode_round_step(height, round, Step::Propose.round_step(), -1),
    ));
    reactor.poll(&reactor_switch);
    thread::sleep(Duration::from_millis(50));

    let messages = got.lock().unwrap_or_else(|err| err.into_inner()).clone();
    let pol = messages.iter().find_map(|(_, bytes)| {
        let msg = Message::decode(bytes.as_slice()).ok()?;
        match msg.sum? {
            message::Sum::ProposalPol(pol) => Some(pol),
            _ => None,
        }
    });
    let pol = pol.expect("ProposalPol was not sent");
    assert_eq!(pol.height, height);
    assert_eq!(pol.proposal_pol_round, pol_round);
    assert!(pol.proposal_pol.is_some());
}

/// `ApplyProposalPOLMessage` stores bits only for the peer's current POL round.
#[test]
fn proposal_pol_for_another_round_is_ignored() {
    let reactor = Reactor::new(nodes(1));
    let switch = switch_for(&reactor);
    let mut proposal = Proposal::new(
        1,
        0,
        0,
        BlockId {
            hash: vec![0x11; 32],
            part_set_header: PartSetHeader {
                total: 1,
                hash: vec![0x22; 32],
            },
        },
        Time::from_unix_parts(1_600_000_000, 0),
    );
    proposal.signature = vec![0xab; 64];

    assert!(reactor.handle(
        &switch,
        "peer",
        STATE_CHANNEL,
        &encode_round_step(1, 0, Step::Propose.round_step(), -1),
    ));
    assert!(reactor.handle(&switch, "peer", DATA_CHANNEL, &encode_proposal(&proposal)));
    assert!(reactor.proposal_pol_bits("peer").is_none());

    assert!(reactor.handle(
        &switch,
        "peer",
        DATA_CHANNEL,
        &encode_proposal_pol(1, 1, &bits_with(0)),
    ));
    assert!(
        reactor.proposal_pol_bits("peer").is_none(),
        "a POL for another round was stored"
    );

    let kept = bits_with(1);
    assert!(reactor.handle(
        &switch,
        "peer",
        DATA_CHANNEL,
        &encode_proposal_pol(1, 0, &kept),
    ));
    assert_eq!(reactor.proposal_pol_bits("peer").as_ref(), Some(&kept));
}

fn tagged_block_id(tag: u8) -> BlockId {
    BlockId {
        hash: vec![tag; 32],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![tag.wrapping_add(1); 32],
        },
    }
}

fn encode_maj23(height: i64, round: i32, kind: SignedMsgType, block_id: &BlockId) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::VoteSetMaj23(consensus::VoteSetMaj23 {
            height,
            round,
            r#type: kind as i32,
            block_id: Some(block_id.to_proto()),
        })),
    }
    .encode_to_vec()
}

fn maj23_messages(messages: &[(u8, Vec<u8>)]) -> Vec<consensus::VoteSetMaj23> {
    messages
        .iter()
        .filter_map(|(ch_id, bytes)| {
            if *ch_id != STATE_CHANNEL {
                return None;
            }
            let msg = Message::decode(bytes.as_slice()).ok()?;
            match msg.sum? {
                message::Sum::VoteSetMaj23(msg) => Some(msg),
                _ => None,
            }
        })
        .collect()
}

fn wait_for(got: &Captured, ready: impl Fn(&[(u8, Vec<u8>)]) -> bool) -> Vec<(u8, Vec<u8>)> {
    let start = Instant::now();
    loop {
        let messages = got.lock().unwrap_or_else(|err| err.into_inner()).clone();
        if ready(&messages) || start.elapsed() >= Duration::from_millis(500) {
            return messages;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// +2/3 prevotes are announced on every poll, not only the first.
#[test]
fn prevote_majority_is_sent_again_on_the_next_poll() {
    let mut validators = Group::new(nodes(4));
    validators.exchange(None, is_block_data);
    // One validator collects the prevotes. Delivering them to everyone would
    // also produce precommits, and the first poll would commit the height.
    let index = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_none())
        .expect("a validator that is not the proposer");
    validators.exchange(Some(index), is_prevote);
    assert!(
        validators.nodes()[index].prevote_count(0) >= 3,
        "prevotes did not reach +2/3"
    );
    let reactor = Reactor::new(validators.into_nodes());
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    assert!(reactor.handle(
        &reactor_switch,
        "right",
        STATE_CHANNEL,
        &encode_round_step(1, 0, Step::Prevote.round_step(), -1),
    ));

    reactor.poll(&reactor_switch);
    let first = wait_for(&got, |messages| !maj23_messages(messages).is_empty());
    let first_prevotes: Vec<_> = maj23_messages(&first)
        .into_iter()
        .filter(|msg| msg.r#type == SignedMsgType::Prevote as i32 && msg.round == 0)
        .collect();
    assert_eq!(
        first_prevotes.len(),
        1,
        "first poll did not send VoteSetMaj23"
    );
    assert_eq!(first_prevotes[0].height, 1);
    assert!(first_prevotes[0].block_id.is_some());

    reactor.poll(&reactor_switch);
    let second = wait_for(&got, |messages| {
        maj23_messages(messages)
            .iter()
            .filter(|msg| msg.r#type == SignedMsgType::Prevote as i32 && msg.round == 0)
            .count()
            >= 2
    });
    let prevotes = maj23_messages(&second)
        .into_iter()
        .filter(|msg| msg.r#type == SignedMsgType::Prevote as i32 && msg.round == 0)
        .count();
    assert!(
        prevotes >= 2,
        "the next poll did not send VoteSetMaj23 again, saw {prevotes}"
    );
}

/// A `VoteSetMaj23` is answered with `VoteSetBits` on `0x23`. A second block id drops the peer.
#[test]
fn maj23_replies_with_vote_set_bits_and_a_conflict_drops_the_peer() {
    let reactor = Reactor::new(nodes(1));
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);

    assert!(peer_switch.send(
        "left",
        STATE_CHANNEL,
        &encode_maj23(1, 0, SignedMsgType::Prevote, &tagged_block_id(1)),
    ));
    let messages = wait_for(&got, |messages| {
        messages
            .iter()
            .any(|(ch_id, _)| *ch_id == VOTE_SET_BITS_CHANNEL)
    });
    let bits = messages.iter().find_map(|(ch_id, bytes)| {
        if *ch_id != VOTE_SET_BITS_CHANNEL {
            return None;
        }
        let msg = Message::decode(bytes.as_slice()).ok()?;
        match msg.sum? {
            message::Sum::VoteSetBits(bits) => Some(bits),
            _ => None,
        }
    });
    let bits = bits.expect("VoteSetBits was not sent on 0x23");
    assert_eq!(bits.height, 1);
    assert_eq!(bits.round, 0);
    assert_eq!(bits.r#type, SignedMsgType::Prevote as i32);
    assert_eq!(reactor_switch.peers().len(), 1);

    assert!(peer_switch.send(
        "left",
        STATE_CHANNEL,
        &encode_maj23(1, 0, SignedMsgType::Prevote, &tagged_block_id(2)),
    ));
    let start = Instant::now();
    while reactor_switch.peers().len() == 1 && start.elapsed() < Duration::from_millis(500) {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        reactor_switch.peers().len(),
        0,
        "a second majority block id did not drop the peer"
    );
}

fn block_part_indexes(messages: &[(u8, Vec<u8>)]) -> Vec<u32> {
    messages
        .iter()
        .filter_map(|(ch_id, bytes)| {
            if *ch_id != DATA_CHANNEL {
                return None;
            }
            let msg = Message::decode(bytes.as_slice()).ok()?;
            match msg.sum? {
                message::Sum::BlockPart(part) => part.part.map(|part| part.index),
                _ => None,
            }
        })
        .collect()
}

fn encode_valid_block(height: i64, round: i32, header: &PartSetHeader, bits: &BitArray) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::NewValidBlock(consensus::NewValidBlock {
            height,
            round,
            block_part_set_header: Some(header.to_proto()),
            block_parts: bits.to_proto(),
            is_commit: false,
        })),
    }
    .encode_to_vec()
}

fn part_bits(total: u32, filled: bool) -> BitArray {
    let width = i64::from(total.max(1));
    let mut bits = BitArray::new(width).expect("bits");
    if filled {
        for index in 0..width {
            bits.set_index(index, true);
        }
    }
    bits
}

fn peer_at_proposal(
    reactor: &Reactor<Exec, Check>,
    switch: &Switch,
    header: &PartSetHeader,
    bits: &BitArray,
) {
    assert!(reactor.handle(
        switch,
        "right",
        STATE_CHANNEL,
        &encode_round_step(1, 0, Step::Propose.round_step(), -1),
    ));
    assert!(reactor.handle(
        switch,
        "right",
        STATE_CHANNEL,
        &encode_valid_block(1, 0, header, bits),
    ));
}

/// Each gossip sends one part the peer does not have, then stops once those bits are set.
///
/// `gossip` stays in propose. `poll` would let this lone validator commit the height first.
#[test]
fn one_missing_part_goes_out_per_poll() {
    let validators = nodes(1);
    let proposal = validators[0].queued_proposal().expect("signed proposal");
    let total = proposal.block_id.part_set_header.total;
    assert!(total >= 1, "proposal has no parts");
    let header = proposal.block_id.part_set_header.clone();
    let reactor = Reactor::new(validators);
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    peer_at_proposal(&reactor, &reactor_switch, &header, &part_bits(total, false));

    let total = usize::try_from(total).expect("part count");
    for sent in 1..=total {
        reactor.gossip(&reactor_switch);
        let messages = wait_for(&got, |messages| block_part_indexes(messages).len() >= sent);
        let indexes = block_part_indexes(&messages);
        assert_eq!(
            indexes.len(),
            sent,
            "gossip {sent} did not send exactly one new part, saw {indexes:?}"
        );
        let mut unique = indexes.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            indexes.len(),
            "a part was sent twice: {indexes:?}"
        );
        assert!(
            indexes
                .iter()
                .all(|index| { usize::try_from(*index).expect("part index") < total })
        );
    }
    assert_eq!(reactor.height(), 1);

    reactor.gossip(&reactor_switch);
    thread::sleep(Duration::from_millis(50));
    let indexes = block_part_indexes(&got.lock().unwrap_or_else(|err| err.into_inner()).clone());
    assert_eq!(
        indexes.len(),
        total,
        "another part went out after the peer had them all: {indexes:?}"
    );
}

/// A peer whose part bits are already full is not sent a part it has.
#[test]
fn part_the_peer_already_has_is_not_sent() {
    let validators = nodes(1);
    let proposal = validators[0].queued_proposal().expect("signed proposal");
    let total = proposal.block_id.part_set_header.total;
    let header = proposal.block_id.part_set_header.clone();
    let reactor = Reactor::new(validators);
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    peer_at_proposal(&reactor, &reactor_switch, &header, &part_bits(total, true));

    reactor.gossip(&reactor_switch);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(reactor.height(), 1);
    let indexes = block_part_indexes(&got.lock().unwrap_or_else(|err| err.into_inner()).clone());
    assert!(
        indexes.is_empty(),
        "a part the peer already has was sent: {indexes:?}"
    );
}

fn block_parts(messages: &[(u8, Vec<u8>)]) -> Vec<consensus::BlockPart> {
    messages
        .iter()
        .filter_map(|(ch_id, bytes)| {
            if *ch_id != DATA_CHANNEL {
                return None;
            }
            let msg = Message::decode(bytes.as_slice()).ok()?;
            match msg.sum? {
                message::Sum::BlockPart(part) => Some(part),
                _ => None,
            }
        })
        .collect()
}

fn captured(got: &Captured) -> Vec<(u8, Vec<u8>)> {
    got.lock().unwrap_or_else(|err| err.into_inner()).clone()
}

fn note_peer_height(reactor: &Reactor<Exec, Check>, switch: &Switch, peer_id: &str, height: i64) {
    assert!(reactor.handle(
        switch,
        peer_id,
        STATE_CHANNEL,
        &encode_round_step(height, 0, Step::NewHeight.round_step(), -1),
    ));
}

/// Store through `store_height`, then start the next consensus height.
fn ahead_by(store_height: i64) -> (Vec<Node<Exec, Check>>, i64, i64) {
    let mut validators = Group::new(nodes(4));
    validators.run_until_height(store_height);
    let our_height = validators.nodes()[0].height();
    let stored = validators.nodes()[0].store_height();
    (validators.into_nodes(), our_height, stored)
}

/// A peer three heights back still gets one part of that stored block.
///
/// The first gossip only stores the part-set header. The next one sends a part.
#[test]
fn peer_three_heights_behind_receives_one_stored_part() {
    let (validators, our_height, stored) = ahead_by(4);
    assert!(our_height > stored);
    let peer_height = our_height - 3;
    assert!(peer_height >= 1);
    assert!(stored >= peer_height);
    let reactor = Reactor::new(validators);
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    note_peer_height(&reactor, &reactor_switch, "right", peer_height);

    reactor.gossip(&reactor_switch);
    thread::sleep(Duration::from_millis(50));
    assert!(
        block_parts(&captured(&got)).is_empty(),
        "the first gossip sent a part before the part-set header was stored"
    );

    reactor.gossip(&reactor_switch);
    let messages = wait_for(&got, |messages| !block_parts(messages).is_empty());
    thread::sleep(Duration::from_millis(50));
    let parts = block_parts(&messages);
    let later = block_parts(&captured(&got));
    assert_eq!(
        later.len(),
        1,
        "gossip sent more than one catch-up part: {later:?}"
    );
    assert_eq!(parts[0].height, peer_height);
    assert_eq!(reactor.height(), our_height);
}

/// A height below the store base, or equal to our consensus height, gets no part.
#[test]
fn catchup_skips_height_below_base_and_our_height() {
    let (validators, our_height, stored) = ahead_by(4);
    assert!(our_height > stored);
    for node in &validators {
        node.prune_blocks(stored).expect("prune to the store tip");
    }
    let reactor = Reactor::new(validators);
    let reactor_switch = switch_for(&reactor);
    let (low_switch, low_got) = capturing_switch();
    let (here_switch, here_got) = capturing_switch();
    link_ids(&reactor_switch, "node", &low_switch, "low");
    link_ids(&reactor_switch, "node", &here_switch, "here");
    note_peer_height(&reactor, &reactor_switch, "low", 1);
    note_peer_height(&reactor, &reactor_switch, "here", our_height);

    reactor.gossip(&reactor_switch);
    reactor.gossip(&reactor_switch);
    thread::sleep(Duration::from_millis(50));
    assert!(
        block_parts(&captured(&low_got)).is_empty(),
        "a height below the store base was sent"
    );
    assert!(
        block_parts(&captured(&here_got)).is_empty(),
        "our own consensus height was sent as catch-up"
    );
}

#[derive(Clone, Debug)]
struct SeenVote {
    height: i64,
    kind: i32,
    index: i32,
}

fn seen_votes(messages: &[(u8, Vec<u8>)]) -> Vec<SeenVote> {
    messages
        .iter()
        .filter_map(|(ch_id, bytes)| {
            if *ch_id != VOTE_CHANNEL {
                return None;
            }
            let msg = Message::decode(bytes.as_slice()).ok()?;
            let vote = match msg.sum? {
                message::Sum::Vote(vote) => vote.vote?,
                _ => return None,
            };
            Some(SeenVote {
                height: vote.height,
                kind: vote.r#type,
                index: vote.validator_index,
            })
        })
        .collect()
}

fn encode_has_vote(height: i64, round: i32, kind: SignedMsgType, index: i32) -> Vec<u8> {
    Message {
        sum: Some(message::Sum::HasVote(consensus::HasVote {
            height,
            round,
            r#type: kind as i32,
            index,
        })),
    }
    .encode_to_vec()
}

/// A collector that has every prevote, still at height 1.
fn node_with_prevotes() -> Node<Exec, Check> {
    let mut validators = Group::new(nodes(4));
    validators.exchange(None, is_block_data);
    let index = validators
        .nodes()
        .iter()
        .position(|node| node.queued_proposal().is_none())
        .expect("a validator that is not the proposer");
    validators.exchange(Some(index), is_prevote);
    let mut nodes = validators.into_nodes();
    nodes.swap_remove(index)
}

/// At `NewHeight` the last commit goes out before a prevote for the current height.
#[test]
fn new_height_sends_last_commit_before_a_prevote() {
    let mut validators = Group::new(nodes(4));
    validators.run_until_height(1);
    validators.exchange(None, is_block_data);
    // Each validator keeps its own prevote. Sharing them would commit height 2.
    validators.pump(2, |_| false);
    let our_height = validators.nodes()[0].height();
    let reactor = Reactor::new(validators.into_nodes());
    assert_eq!(our_height, 2);
    assert!(
        reactor.prevote_count(0) > 0,
        "the current height has no prevote to order against"
    );
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    assert!(reactor.handle(
        &reactor_switch,
        "right",
        STATE_CHANNEL,
        &encode_round_step(our_height, 0, Step::NewHeight.round_step(), 0),
    ));

    reactor.gossip(&reactor_switch);
    let messages = wait_for(&got, |messages| !seen_votes(messages).is_empty());
    thread::sleep(Duration::from_millis(50));
    let votes = seen_votes(&captured(&got));
    assert_eq!(
        votes.len(),
        1,
        "one gossip sent more than one vote: {votes:?}"
    );
    assert_eq!(votes[0].kind, SignedMsgType::Precommit as i32);
    assert_eq!(votes[0].height, our_height - 1);
    assert_eq!(seen_votes(&messages)[0].height, our_height - 1);
}

/// A vote the peer has acked is not sent on the next pass. One pass sends one vote.
#[test]
fn acked_vote_is_not_sent_on_the_next_poll() {
    let reactor = Reactor::new(vec![node_with_prevotes()]);
    assert_eq!(reactor.height(), 1);
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    assert!(reactor.handle(
        &reactor_switch,
        "right",
        STATE_CHANNEL,
        &encode_round_step(1, 0, Step::Prevote.round_step(), -1),
    ));

    reactor.gossip(&reactor_switch);
    let first = wait_for(&got, |messages| !seen_votes(messages).is_empty());
    let first_votes = seen_votes(&first);
    assert_eq!(first_votes.len(), 1, "the first pass did not send one vote");
    assert_eq!(first_votes[0].kind, SignedMsgType::Prevote as i32);
    let acked = first_votes[0].index;
    assert!(reactor.handle(
        &reactor_switch,
        "right",
        STATE_CHANNEL,
        &encode_has_vote(1, 0, SignedMsgType::Prevote, acked),
    ));

    reactor.gossip(&reactor_switch);
    let second = wait_for(&got, |messages| seen_votes(messages).len() >= 2);
    thread::sleep(Duration::from_millis(50));
    let votes = seen_votes(&captured(&got));
    assert_eq!(
        votes.len(),
        2,
        "the next pass did not send exactly one vote"
    );
    assert_ne!(votes[1].index, acked, "the acked vote was sent again");
    assert_eq!(seen_votes(&second)[1].index, votes[1].index);
}

/// A peer three heights behind still gets one precommit from the stored block commit.
#[test]
fn peer_three_heights_behind_receives_a_commit_precommit() {
    let (validators, our_height, stored) = ahead_by(4);
    let peer_height = our_height - 3;
    assert!(peer_height >= 1);
    assert!(stored >= peer_height);
    assert!(our_height >= peer_height + 2);
    let reactor = Reactor::new(validators);
    let reactor_switch = switch_for(&reactor);
    let (peer_switch, got) = capturing_switch();
    link(&reactor_switch, &peer_switch);
    note_peer_height(&reactor, &reactor_switch, "right", peer_height);

    reactor.gossip(&reactor_switch);
    let messages = wait_for(&got, |messages| !seen_votes(messages).is_empty());
    thread::sleep(Duration::from_millis(50));
    let votes = seen_votes(&captured(&got));
    assert_eq!(
        votes.len(),
        1,
        "a gap of three sent more than one vote: {votes:?}"
    );
    assert_eq!(votes[0].kind, SignedMsgType::Precommit as i32);
    assert_eq!(votes[0].height, peer_height);
    assert_eq!(seen_votes(&messages)[0].height, peer_height);
}
