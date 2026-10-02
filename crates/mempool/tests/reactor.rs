//! Two switches gossiping mempool txs. No WAL and no consensus height gate.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eld_tendermint_config::MempoolConfig;
use eld_tendermint_crypto::PrivKey;
use eld_tendermint_mempool::{App, Mempool, Reactor, channel_descriptors};
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_proto::abci::{RequestCheckTx, ResponseCheckTx};
use eld_tendermint_proto::mempool::{Message, message};
use eld_tendermint_types::Tx;
use prost::Message as ProstMessage;
use prost::bytes::Bytes;

use eld_tendermint_mempool::MEMPOOL_CHANNEL;

struct Code(u32);

impl App for Code {
    fn check_tx(&mut self, _: RequestCheckTx) -> ResponseCheckTx {
        ResponseCheckTx {
            code: self.0,
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

fn reactor(code: u32) -> Reactor<Code> {
    let pool = Mempool::new(MempoolConfig::test_config(), Code(code)).expect("v0 mempool");
    Reactor::new(pool)
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

fn descriptors() -> Vec<eld_tendermint_p2p::ChannelDescriptor> {
    channel_descriptors(MempoolConfig::test_config().max_tx_bytes)
}

fn attach<A>(reactor: &Reactor<A>, seen: Arc<Mutex<Vec<Vec<u8>>>>) -> Arc<Switch>
where
    A: App + Send + 'static,
{
    let switch = Arc::new(Switch::new());
    let callback_switch = Arc::clone(&switch);
    let callback_reactor = reactor.clone();
    switch
        .add_reactor("mempool", descriptors(), move |peer_id, ch_id, bytes| {
            seen.lock()
                .unwrap_or_else(|err| err.into_inner())
                .push(bytes.clone());
            if !callback_reactor.handle(peer_id, ch_id, &bytes) {
                callback_switch.stop_peer(peer_id);
            }
        })
        .expect("reactor");
    switch
}

fn connect(left: &Switch, left_peer: &str, right: &Switch, right_peer: &str) {
    let (conn_left, conn_right) = secret_pair();
    left.add_peer(conn_left, left_peer).expect("left peer");
    right.add_peer(conn_right, right_peer).expect("right peer");
}

fn tx_bytes(payloads: &[Vec<u8>]) -> Vec<Vec<u8>> {
    payloads
        .iter()
        .filter_map(|payload| {
            let message = Message::decode(payload.as_slice()).ok()?;
            match message.sum {
                Some(message::Sum::Txs(txs)) => Some(txs.txs),
                None => None,
            }
        })
        .flatten()
        .collect()
}

#[test]
fn checked_txs_are_reaped_in_order_and_not_echoed() {
    let left = reactor(0);
    let right = reactor(0);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let left_switch = attach(&left, Arc::clone(&seen));
    let right_switch = attach(&right, Arc::new(Mutex::new(Vec::new())));
    connect(&left_switch, "right", &right_switch, "left");

    let first = Tx::new(b"tx-one".to_vec());
    let second = Tx::new(b"tx-two".to_vec());
    left.check_tx(&first).expect("first tx");
    left.check_tx(&second).expect("second tx");

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        if right.reap() == [first.clone(), second.clone()] {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(right.reap(), vec![first.clone(), second.clone()]);

    for _ in 0..20 {
        left.poll(&left_switch);
        right.poll(&right_switch);
        thread::sleep(Duration::from_millis(5));
    }
    let echoed = tx_bytes(&seen.lock().unwrap_or_else(|err| err.into_inner()));
    assert!(
        !echoed
            .iter()
            .any(|tx| tx.as_slice() == first.as_bytes() || tx.as_slice() == second.as_bytes()),
        "echoed {echoed:?}"
    );
}

#[test]
fn rejected_tx_is_not_pooled_and_peer_stays() {
    let left = reactor(0);
    let right = reactor(1);
    let left_switch = attach(&left, Arc::new(Mutex::new(Vec::new())));
    let right_switch = attach(&right, Arc::new(Mutex::new(Vec::new())));
    connect(&left_switch, "right", &right_switch, "left");

    let tx = Tx::new(b"nope".to_vec());
    left.check_tx(&tx).expect("local check");

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(right.reap().is_empty());
    assert_eq!(left_switch.peers().len(), 1);
}

#[test]
fn malformed_message_stops_only_that_peer() {
    let pool = reactor(0);
    let receiver = attach(&pool, Arc::new(Mutex::new(Vec::new())));

    let bad = Switch::new();
    bad.add_reactor("mempool", descriptors(), |_, _, _| {})
        .expect("bad reactor");
    let good = Switch::new();
    good.add_reactor("mempool", descriptors(), |_, _, _| {})
        .expect("good reactor");
    connect(&receiver, "bad", &bad, "recv");
    connect(&receiver, "good", &good, "recv");

    assert!(bad.send("recv", MEMPOOL_CHANNEL, b"\x0a"));
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        let ids: Vec<String> = receiver.peers().into_iter().map(|peer| peer.id).collect();
        if ids == ["good"] {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let ids: Vec<String> = receiver.peers().into_iter().map(|peer| peer.id).collect();
    panic!("peers still {ids:?}");
}
