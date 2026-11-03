#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Two switches on the content channel. One push, one receipt.

use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use eld_tendermint_content::{CONTENT_CHANNEL, MAX_BLOB_BYTES, Reactor, channel_descriptors};
use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_proto::content::{self, ContentPush, ContentPushAck, Message};

const CONTENT_ID: &str = "abababababababababababababababababababababababababababababababab";

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

fn attach(reactor: &Reactor) -> std::sync::Arc<Switch> {
    let switch = std::sync::Arc::new(Switch::new());
    let callback_switch = std::sync::Arc::clone(&switch);
    let callback_reactor = reactor.clone();
    switch
        .add_reactor(
            "content",
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

fn connect(left: &Switch, left_peer: &str, right: &Switch, right_peer: &str) {
    let (conn_left, conn_right) = secret_pair();
    left.add_peer(conn_left, left_peer).expect("left peer");
    right.add_peer(conn_right, right_peer).expect("right peer");
}

fn push(request_id: u64, blob: Vec<u8>) -> Message {
    Message {
        sum: Some(content::message::Sum::ContentPush(ContentPush {
            request_id,
            content_id: CONTENT_ID.to_owned(),
            blob,
        })),
    }
}

#[test]
fn push_is_answered_with_a_receipt() {
    let left = Reactor::new();
    let right = Reactor::new();
    let left_switch = attach(&left);
    let right_switch = attach(&right);
    connect(&left_switch, "right", &right_switch, "left");

    assert!(left.queue("right", &push(7, b"hello".to_vec())));

    let start = std::time::Instant::now();
    let mut got_push = false;
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        for inbound in right.drain_inbound() {
            assert_eq!(inbound.peer_id, "left");
            assert_eq!(inbound.message, push(7, b"hello".to_vec()));
            got_push = true;
        }
        if got_push {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(got_push, "push did not arrive");

    let ack = Message {
        sum: Some(content::message::Sum::ContentPushAck(ContentPushAck {
            request_id: 7,
            content_id: CONTENT_ID.to_owned(),
            provider: vec![0x22; 20],
        })),
    };
    assert!(right.queue("left", &ack));

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        let inbound = left.drain_inbound();
        if inbound.iter().any(|item| item.message == ack) {
            assert!(inbound.iter().all(|item| item.peer_id == "right"));
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("receipt did not arrive");
}

#[test]
fn malformed_message_stops_only_that_peer() {
    let receiver = Reactor::new();
    let switch = attach(&receiver);

    let bad = Switch::new();
    bad.add_reactor("content", channel_descriptors(), |_, _, _| {})
        .expect("bad reactor");
    let good = Switch::new();
    good.add_reactor("content", channel_descriptors(), |_, _, _| {})
        .expect("good reactor");
    connect(&switch, "bad", &bad, "recv");
    connect(&switch, "good", &good, "recv");

    assert!(bad.send("recv", CONTENT_CHANNEL, b"\x0a"));
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        let ids: Vec<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        if ids == ["good"] {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let ids: Vec<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
    panic!("peers still {ids:?}");
}

#[test]
fn a_blob_over_one_mib_is_not_queued() {
    let reactor = Reactor::new();
    assert!(!reactor.queue("peer", &push(1, vec![0; MAX_BLOB_BYTES + 1])));
    assert!(reactor.queue("peer", &push(1, vec![0; 4])));
}
