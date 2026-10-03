//! TCP dial, accept, address book, and PEX.

use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::Duration;

use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{
    AddrBook, ChannelDescriptor, NetAddress, NodeKey, PexReactor, Switch, pex_channel_descriptors,
};
use eld_tendermint_proto::p2p::{Message, PexAddrs, message};
use eld_tendermint_types::set_log_capture;
use prost::Message as ProstMessage;

fn node_key() -> NodeKey {
    NodeKey {
        priv_key: PrivKey::generate(),
    }
}

fn state_channel() -> ChannelDescriptor {
    ChannelDescriptor {
        id: 0x20,
        priority: 6,
        send_queue_capacity: 100,
        recv_message_capacity: 1024 * 1024,
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

fn listen(switch: &Switch, key: &NodeKey) -> std::net::SocketAddr {
    switch.listen(key, "127.0.0.1:0").expect("listen")
}

#[test]
fn persistent_peer_dial_delivers_channel_message() {
    let key_a = node_key();
    let key_b = node_key();
    let switch_a = Switch::new();
    let switch_b = Switch::new();
    let got = Arc::new(Mutex::new(None));
    let got_b = Arc::clone(&got);
    switch_a
        .add_reactor("state", vec![state_channel()], |_, _, _| {})
        .expect("reactor a");
    switch_b
        .add_reactor("state", vec![state_channel()], move |_, _, bytes| {
            *got_b.lock().unwrap_or_else(|err| err.into_inner()) = Some(bytes);
        })
        .expect("reactor b");

    let bound = listen(&switch_b, &key_b);
    let peers = format!("{}@{bound}", key_b.id().expect("id"));
    switch_a.dial_persistent(&key_a, &peers).expect("dial");

    wait_until(|| switch_a.peers().len() == 1 && switch_b.peers().len() == 1);
    let payload = vec![0x11u8; 2000];
    assert!(switch_a.broadcast(0x20, &payload));
    wait_until(|| got.lock().unwrap_or_else(|err| err.into_inner()).as_ref() == Some(&payload));
}

#[test]
fn pex_addrs_are_stored_and_dialed() {
    let key_a = node_key();
    let key_b = node_key();
    let key_c = node_key();
    let switch_a = Arc::new(Switch::new());
    let switch_b = Arc::new(Switch::new());
    let switch_c = Switch::new();
    let book_path = std::env::temp_dir().join(format!("eld-pex-book-{}", std::process::id()));
    let _ = std::fs::remove_file(&book_path);
    let reactor = PexReactor::new(AddrBook::open(&book_path), key_b.clone());

    switch_a
        .add_reactor("pex", pex_channel_descriptors(), |_, _, _| {})
        .expect("reactor a");
    let weak: Weak<Switch> = Arc::downgrade(&switch_b);
    let callback_reactor = reactor.clone();
    switch_b
        .add_reactor(
            "pex",
            pex_channel_descriptors(),
            move |peer_id, _, bytes| {
                let Some(switch) = weak.upgrade() else {
                    return;
                };
                if !callback_reactor.handle(&switch, peer_id, &bytes) {
                    switch.stop_peer(peer_id);
                }
            },
        )
        .expect("reactor b");

    let bound_b = listen(&switch_b, &key_b);
    let bound_c = listen(&switch_c, &key_c);
    let peers = format!("{}@{bound_b}", key_b.id().expect("id b"));
    switch_a.dial_persistent(&key_a, &peers).expect("dial b");
    wait_until(|| switch_a.peers().len() == 1 && switch_b.peers().len() == 1);

    let c_id = key_c.id().expect("id c");
    let addr = NetAddress::parse(&format!("{c_id}@{bound_c}")).expect("addr c");
    let bytes = Message {
        sum: Some(message::Sum::PexAddrs(PexAddrs {
            addrs: vec![addr.to_proto()],
        })),
    }
    .encode_to_vec();
    let b_id = key_b.id().expect("id b");
    assert!(switch_a.send(&b_id, eld_tendermint_p2p::PEX_CHANNEL, &bytes));

    wait_until(|| {
        reactor.get(&c_id).is_some()
            && switch_b.peers().iter().any(|peer| peer.id == c_id)
            && switch_c.peers().len() == 1
    });
    let _ = std::fs::remove_file(&book_path);
}

#[test]
fn refused_tcp_dial_logs_dial_error() {
    let capture = Arc::new(Mutex::new(String::new()));
    set_log_capture(Some(Arc::clone(&capture)));
    let key = node_key();
    let switch = Switch::new();
    let addr = NetAddress::parse(&format!("{}@127.0.0.1:1", "ab".repeat(20))).expect("addr");
    let err = switch.dial_address(&key, &addr);
    let logs = capture
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone();
    set_log_capture(None);
    assert!(err.is_err(), "refused dial should fail");
    assert!(logs.contains("Dialing peer"), "{logs}");
    assert!(logs.contains("address=127.0.0.1:1"), "{logs}");
    assert!(logs.contains("Error dialing peer"), "{logs}");
    assert!(
        !logs.lines().any(|line| {
            line.contains("127.0.0.1:1") && line.contains("Secret handshake failed")
        }),
        "{logs}"
    );
}

#[test]
fn mismatched_dial_id_is_dropped() {
    let key_a = node_key();
    let key_b = node_key();
    let switch_a = Switch::new();
    let switch_b = Switch::new();
    let bound = listen(&switch_b, &key_b);
    let fake = "ab".repeat(20);
    let peers = format!("{fake}@{bound}");
    switch_a
        .dial_persistent(&key_a, &peers)
        .expect("dial returns after the mismatch");
    thread::sleep(Duration::from_millis(50));
    assert!(switch_a.peers().is_empty());
}

#[test]
fn addrbook_reloads_after_mark_good() {
    let path = std::env::temp_dir().join(format!(
        "eld-addrbook-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|dur| dur.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&path);
    let id = "cd".repeat(20);
    let src = "ef".repeat(20);
    let addr = NetAddress::parse(&format!("{id}@127.0.0.1:26657")).expect("addr");
    let src_addr = NetAddress::parse(&format!("{src}@127.0.0.1:1")).expect("src");

    let mut book = AddrBook::open(&path);
    book.add(addr, src_addr);
    book.mark_good(&id);
    book.save().expect("save");

    let text = std::fs::read_to_string(&path).expect("file");
    assert!(
        !text.contains("127.0.0.1"),
        "ip is base64, not a dotted string"
    );
    assert_eq!(book.key().len(), 24);

    let mut loaded = AddrBook::open(&path);
    let known = loaded.get(&id).expect("reloaded").clone();
    assert_eq!(known.addr.port, 26657);
    assert_eq!(known.bucket_type, 0x02);
    assert_eq!(known.attempts, 0);
    assert_eq!(loaded.key(), book.key());

    loaded.mark_bad(&known.addr);
    loaded.save().expect("save after bad");
    assert!(AddrBook::open(&path).get(&id).is_none());
    let _ = std::fs::remove_file(&path);
}
