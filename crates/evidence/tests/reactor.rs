//! Evidence gossip. A peer is not echoed, and a bad vote does not disconnect it.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eld_tendermint_crypto::PrivKey;
use eld_tendermint_evidence::{EVIDENCE_CHANNEL, Pool, Reactor, channel_descriptors};
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_proto::types::EvidenceList as ProtoList;
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_state::make_genesis_state;
use eld_tendermint_store::MemDb;
use eld_tendermint_types::{
    BlockId, ChainId, DuplicateVoteEvidence, GenesisDoc, GenesisValidator, PartSetHeader, Time,
    Vote,
};
use prost::Message;

fn evidence() -> (eld_tendermint_state::State, DuplicateVoteEvidence) {
    let key = PrivKey::generate();
    let pub_key = key.public_key().expect("pub key");
    let mut genesis = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: None,
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key,
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    let state = make_genesis_state(&mut genesis).expect("genesis");
    let address = pub_key.address().to_vec();
    let vote = |byte: u8| {
        let mut vote = Vote {
            vote_type: SignedMsgType::Prevote,
            height: 1,
            round: 0,
            block_id: BlockId {
                hash: vec![byte; 32],
                part_set_header: PartSetHeader {
                    total: 1,
                    hash: vec![byte.wrapping_add(1); 32],
                },
            },
            timestamp: Time::from_unix_parts(1_600_000_000, 0),
            validator_address: address.clone(),
            validator_index: 0,
            signature: Vec::new(),
        };
        vote.sign(&key, "test-chain").expect("sign");
        vote
    };
    let evidence = DuplicateVoteEvidence::new(
        vote(1),
        vote(2),
        Time::from_unix_parts(1_600_000_000, 0),
        &state.validators,
    )
    .expect("evidence");
    (state, evidence)
}

fn encode(evidence: &DuplicateVoteEvidence) -> Vec<u8> {
    ProtoList {
        evidence: vec![evidence.to_evidence_proto()],
    }
    .encode_to_vec()
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

fn switch_for(reactor: &Reactor<MemDb>) -> Arc<Switch> {
    let switch = Arc::new(Switch::new());
    let callback_switch = Arc::clone(&switch);
    let callback_reactor = reactor.clone();
    switch
        .add_reactor(
            "evidence",
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

#[test]
fn peer_evidence_is_not_echoed() {
    let (state, evidence) = evidence();
    let max_bytes = state.consensus_params.evidence.max_bytes;
    let sender_pool = Pool::new(MemDb::new(), &state);
    sender_pool.add(evidence).expect("add");
    let receiver_pool = Pool::new(MemDb::new(), &state);
    let sender = Reactor::new(sender_pool);
    let receiver = Reactor::new(receiver_pool.clone());
    let sender_switch = switch_for(&sender);
    let receiver_switch = switch_for(&receiver);
    link(&sender_switch, &receiver_switch);

    let start = Instant::now();
    while receiver_pool.pending(max_bytes).is_empty() && start.elapsed() < Duration::from_secs(2) {
        sender.poll(&sender_switch);
        receiver.poll(&receiver_switch);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(receiver_pool.pending(max_bytes).len(), 1);
    let before = sender.messages_received();
    let again = Instant::now();
    while again.elapsed() < Duration::from_millis(50) {
        receiver.poll(&receiver_switch);
        sender.poll(&sender_switch);
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(sender.messages_received(), before);
}

#[test]
fn failed_verify_is_not_stored_and_peer_stays() {
    let (state, mut evidence) = evidence();
    let byte = evidence.vote_a.signature.last_mut().expect("signature");
    *byte ^= 0xff;
    let pool = Pool::new(MemDb::new(), &state);
    let reactor = Reactor::new(pool.clone());
    let receiver = switch_for(&reactor);
    let sender = Switch::new();
    sender
        .add_reactor("evidence", channel_descriptors(), |_, _, _| {})
        .expect("sender");
    link(&receiver, &sender);
    assert!(sender.send("left", EVIDENCE_CHANNEL, &encode(&evidence)));
    thread::sleep(Duration::from_millis(30));
    assert!(
        pool.pending(state.consensus_params.evidence.max_bytes)
            .is_empty()
    );
    assert_eq!(receiver.peers().len(), 1);
}

#[test]
fn malformed_evidence_stops_the_peer() {
    let (state, _) = evidence();
    let pool = Pool::new(MemDb::new(), &state);
    let reactor = Reactor::new(pool.clone());
    let receiver = switch_for(&reactor);
    let sender = Switch::new();
    sender
        .add_reactor("evidence", channel_descriptors(), |_, _, _| {})
        .expect("sender");
    link(&receiver, &sender);
    assert!(sender.send("left", EVIDENCE_CHANNEL, &[0xff, 0xff, 0xff, 0xff]));
    thread::sleep(Duration::from_millis(30));
    assert!(
        pool.pending(state.consensus_params.evidence.max_bytes)
            .is_empty()
    );
    assert!(receiver.peers().is_empty());
}
