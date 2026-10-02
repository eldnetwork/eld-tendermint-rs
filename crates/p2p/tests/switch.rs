//! Two in-process switches on a secret connection.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{ChannelDescriptor, SecretConnection, Switch, make_secret_connection};
use eld_tendermint_proto::p2p::{Packet, PacketPing, packet};
use prost::Message;

fn secret_pair() -> (SecretConnection<UnixStream>, SecretConnection<UnixStream>) {
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

fn state_channel(recv_message_capacity: usize) -> ChannelDescriptor {
    ChannelDescriptor {
        id: 0x20,
        priority: 6,
        send_queue_capacity: 100,
        recv_message_capacity,
    }
}

fn wait_until(mut pred: impl FnMut() -> bool) {
    for _ in 0..100 {
        if pred() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out");
}

fn read_packet(reader: &mut impl Read) -> Packet {
    let len = read_uvarint(reader);
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).expect("packet bytes");
    Packet::decode(buf.as_slice()).expect("packet")
}

fn read_uvarint(reader: &mut impl Read) -> usize {
    let mut value = 0u64;
    let mut shift = 0;
    for i in 0..10 {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).expect("varint");
        let byte = byte[0];
        if byte < 0x80 {
            if i == 9 && byte > 1 {
                panic!("invalid varint");
            }
            return usize::try_from(value | u64::from(byte) << shift).expect("length");
        }
        value |= u64::from(byte & 0x7f) << shift;
        shift += 7;
    }
    panic!("invalid varint");
}

#[test]
fn peers_exchange_reassembled_message() {
    let (conn_a, conn_b) = secret_pair();
    let got = Arc::new(Mutex::new(None));
    let hits = Arc::new(AtomicUsize::new(0));
    let got_cb = Arc::clone(&got);
    let hits_cb = Arc::clone(&hits);

    let recv = Switch::new();
    recv.add_reactor(
        "state",
        vec![state_channel(1024 * 1024)],
        move |peer_id, ch, bytes| {
            assert_eq!(peer_id, "peer-a");
            assert_eq!(ch, 0x20);
            hits_cb.fetch_add(1, Ordering::SeqCst);
            *got_cb.lock().expect("lock") = Some(bytes);
        },
    )
    .expect("reactor");
    recv.add_peer(conn_b, "peer-a").expect("peer");

    let send = Switch::new();
    send.add_reactor("state", vec![state_channel(1024 * 1024)], |_, _, _| {})
        .expect("reactor");
    send.add_peer(conn_a, "peer-b").expect("peer");

    let payload = vec![0xABu8; 2000];
    assert!(send.broadcast(0x20, &payload));
    wait_until(|| got.lock().expect("lock").is_some());
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(
        got.lock().expect("lock").as_deref(),
        Some(payload.as_slice())
    );
    assert_eq!(recv.peers().len(), 1);
    assert_eq!(recv.peers()[0].id, "peer-a");
    assert_eq!(recv.peers()[0].remote_addr, "");
}

#[test]
fn packet_ping_receives_pong() {
    let (mut raw, peer) = secret_pair();
    let sw = Switch::new();
    sw.add_reactor("state", vec![state_channel(1024 * 1024)], |_, _, _| {})
        .expect("reactor");
    sw.add_peer(peer, "peer").expect("peer");

    let ping = Packet {
        sum: Some(packet::Sum::PacketPing(PacketPing {})),
    };
    raw.write_all(&ping.encode_length_delimited_to_vec())
        .expect("write ping");
    let pong = read_packet(&mut raw);
    assert!(matches!(pong.sum, Some(packet::Sum::PacketPong(_))));
    assert_eq!(sw.peers().len(), 1);
}

#[test]
fn unknown_channel_send_keeps_peer_up() {
    let (conn_a, conn_b) = secret_pair();
    let got = Arc::new(Mutex::new(None));
    let got_cb = Arc::clone(&got);
    let recv = Switch::new();
    recv.add_reactor(
        "state",
        vec![state_channel(1024 * 1024)],
        move |_peer, ch, bytes| {
            assert_eq!(ch, 0x20);
            *got_cb.lock().expect("lock") = Some(bytes);
        },
    )
    .expect("reactor");
    recv.add_peer(conn_b, "peer-a").expect("peer");

    let send = Switch::new();
    send.add_reactor("state", vec![state_channel(1024 * 1024)], |_, _, _| {})
        .expect("reactor");
    send.add_peer(conn_a, "peer-b").expect("peer");

    assert!(!send.broadcast(0x01, b"nope"));
    assert_eq!(send.peers().len(), 1);
    assert!(send.broadcast(0x20, b"yes"));
    wait_until(|| got.lock().expect("lock").is_some());
    assert_eq!(
        got.lock().expect("lock").as_deref(),
        Some(b"yes".as_slice())
    );
    assert_eq!(recv.peers().len(), 1);
}

#[test]
fn message_over_recv_capacity_stops_peer() {
    let (conn_a, conn_b) = secret_pair();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_cb = Arc::clone(&hits);
    let recv = Switch::new();
    recv.add_reactor("state", vec![state_channel(8)], move |_, _, _| {
        hits_cb.fetch_add(1, Ordering::SeqCst);
    })
    .expect("reactor");
    recv.add_peer(conn_b, "peer-a").expect("peer");

    let send = Switch::new();
    send.add_reactor("state", vec![state_channel(1024 * 1024)], |_, _, _| {})
        .expect("reactor");
    send.add_peer(conn_a, "peer-b").expect("peer");

    assert!(send.broadcast(0x20, &[0u8; 9]));
    wait_until(|| recv.peers().is_empty());
    assert_eq!(hits.load(Ordering::SeqCst), 0);
}
