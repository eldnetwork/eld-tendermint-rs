#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Two switches on the content channel. One push, one receipt.
//! A message that does not decode stops that peer. An app refusal does not.

use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use eld_tendermint_content::{
    App, CONTENT_CHANNEL, ContentSendQueue, Error, MAX_BLOB_BYTES, Reactor, channel_descriptors,
};
use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_proto::content::{
    self, Announce, CapacityChallenge, CapacityChallengeResponse, ContentPush, ContentPushAck,
    ContentRequest, ContentResponse, Message,
};
use eld_tendermint_types::set_log_capture;
use prost::Message as ProstMessage;

const CONTENT_ID: &str = "abababababababababababababababababababababababababababababababab";

#[derive(Clone, Copy)]
enum Mode {
    Answer,
    Reject,
    Missing,
    Oversize,
}

#[derive(Clone)]
struct Recording {
    mode: Mode,
    seen: Arc<Mutex<Vec<(String, Message)>>>,
}

impl Recording {
    fn answer() -> Self {
        Self::new(Mode::Answer)
    }

    fn reject() -> Self {
        Self::new(Mode::Reject)
    }

    fn missing() -> Self {
        Self::new(Mode::Missing)
    }

    fn oversize() -> Self {
        Self::new(Mode::Oversize)
    }

    fn new(mode: Mode) -> Self {
        Self {
            mode,
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn seen(&self) -> Vec<(String, Message)> {
        self.seen.lock().unwrap().clone()
    }

    fn record(&self, peer_id: &str, sum: content::message::Sum) {
        self.seen
            .lock()
            .unwrap()
            .push((peer_id.to_owned(), Message { sum: Some(sum) }));
    }
}

fn rejected() -> Error {
    Error::Rejected {
        reason: "refused".to_owned(),
    }
}

impl App for Recording {
    fn handle_content_push(
        &self,
        peer_id: &str,
        msg: ContentPush,
    ) -> Result<ContentPushAck, Error> {
        let ack = ContentPushAck {
            request_id: msg.request_id,
            content_id: msg.content_id.clone(),
            provider: vec![0x22; 20],
        };
        self.record(peer_id, content::message::Sum::ContentPush(msg));
        match self.mode {
            Mode::Answer => Ok(ack),
            Mode::Reject | Mode::Missing | Mode::Oversize => Err(rejected()),
        }
    }

    fn handle_content_request(
        &self,
        peer_id: &str,
        msg: ContentRequest,
    ) -> Result<Option<ContentResponse>, Error> {
        let response = ContentResponse {
            request_id: msg.request_id,
            content_id: msg.content_id.clone(),
            blob: b"blob".to_vec(),
        };
        self.record(peer_id, content::message::Sum::ContentRequest(msg));
        match self.mode {
            Mode::Answer => Ok(Some(response)),
            Mode::Missing => Ok(None),
            Mode::Oversize => Ok(Some(ContentResponse {
                request_id: response.request_id,
                content_id: response.content_id,
                blob: vec![0; MAX_BLOB_BYTES + 1],
            })),
            Mode::Reject => Err(rejected()),
        }
    }

    fn handle_capacity_challenge(
        &self,
        peer_id: &str,
        msg: CapacityChallenge,
    ) -> Result<CapacityChallengeResponse, Error> {
        let response = CapacityChallengeResponse {
            challenge_id: msg.challenge_id.clone(),
            provider: msg.provider.clone(),
            challenger: msg.challenger.clone(),
            block_height: msg.block_height,
            proofs: Vec::new(),
            generated_at: 0,
            provider_pubkey: Vec::new(),
            provider_signature: Vec::new(),
        };
        self.record(peer_id, content::message::Sum::CapacityChallenge(msg));
        match self.mode {
            Mode::Answer => Ok(response),
            Mode::Reject | Mode::Missing | Mode::Oversize => Err(rejected()),
        }
    }

    fn handle_content_push_ack(&self, peer_id: &str, msg: ContentPushAck) -> Result<(), Error> {
        self.record(peer_id, content::message::Sum::ContentPushAck(msg));
        Ok(())
    }

    fn handle_content_response(&self, peer_id: &str, msg: ContentResponse) -> Result<(), Error> {
        self.record(peer_id, content::message::Sum::ContentResponse(msg));
        Ok(())
    }

    fn handle_capacity_challenge_response(
        &self,
        peer_id: &str,
        msg: CapacityChallengeResponse,
    ) -> Result<(), Error> {
        self.record(
            peer_id,
            content::message::Sum::CapacityChallengeResponse(msg),
        );
        Ok(())
    }

    fn handle_announce(&self, peer_id: &str, msg: Announce) -> Result<(), Error> {
        self.record(peer_id, content::message::Sum::Announce(msg));
        Ok(())
    }

    fn capacity_proof_summary(&self, _height: i64) -> Result<Option<Vec<u8>>, Error> {
        Ok(None)
    }
}

fn opened(app: Recording) -> Reactor<Recording> {
    Reactor::from_shared(app, Arc::new(ContentSendQueue::new()))
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

fn attach<A>(reactor: &Reactor<A>) -> std::sync::Arc<Switch>
where
    A: App + 'static,
{
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

fn request(request_id: u64) -> Message {
    Message {
        sum: Some(content::message::Sum::ContentRequest(ContentRequest {
            request_id,
            content_id: CONTENT_ID.to_owned(),
        })),
    }
}

fn challenge() -> Message {
    Message {
        sum: Some(content::message::Sum::CapacityChallenge(
            CapacityChallenge {
                challenge_id: "challenge-1".to_owned(),
                challenger: vec![0x11; 20],
                provider: vec![0x22; 20],
                chunk_indices: vec![0, 3],
                block_height: 42,
                merkle_root: vec![1; 32],
                seed: vec![2; 32],
                expiration_block: 142,
                timestamp: 1_700_000_000,
            },
        )),
    }
}

fn announce() -> Message {
    Message {
        sum: Some(content::message::Sum::Announce(Announce {
            content_id: CONTENT_ID.to_owned(),
        })),
    }
}

fn ack_for(request_id: u64) -> Message {
    Message {
        sum: Some(content::message::Sum::ContentPushAck(ContentPushAck {
            request_id,
            content_id: CONTENT_ID.to_owned(),
            provider: vec![0x22; 20],
        })),
    }
}

#[test]
fn push_is_answered_with_a_receipt() {
    let left_app = Recording::answer();
    let right_app = Recording::answer();
    let left = opened(left_app.clone());
    let right = opened(right_app.clone());
    let left_switch = attach(&left);
    let right_switch = attach(&right);
    connect(&left_switch, "right", &right_switch, "left");

    assert!(left.queue("right", &push(7, b"hello".to_vec())));

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        if left_app
            .seen()
            .iter()
            .any(|(_, message)| *message == ack_for(7))
        {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }

    let pushes: Vec<_> = right_app
        .seen()
        .into_iter()
        .filter(|(_, message)| *message == push(7, b"hello".to_vec()))
        .collect();
    assert_eq!(pushes.len(), 1);
    assert_eq!(pushes[0].0, "left");

    let acks: Vec<_> = left_app
        .seen()
        .into_iter()
        .filter(|(_, message)| *message == ack_for(7))
        .collect();
    assert_eq!(acks.len(), 1, "receipt did not arrive");
    assert_eq!(acks[0].0, "right");
}

#[test]
fn the_node_queue_can_start_a_push_fetch_and_challenge() {
    let outgoing = Arc::new(ContentSendQueue::new());
    let left_app = Recording::answer();
    let right_app = Recording::answer();
    let left = Reactor::from_shared(left_app, Arc::clone(&outgoing));
    let right = opened(right_app.clone());
    let left_switch = attach(&left);
    let right_switch = attach(&right);
    connect(&left_switch, "right", &right_switch, "left");

    assert!(outgoing.push(
        "right",
        &ContentPush {
            request_id: 7,
            content_id: CONTENT_ID.to_owned(),
            blob: b"hello".to_vec(),
        },
    ));
    assert!(outgoing.request(
        "right",
        &ContentRequest {
            request_id: 4,
            content_id: CONTENT_ID.to_owned(),
        },
    ));
    assert!(outgoing.challenge(
        "right",
        &CapacityChallenge {
            challenge_id: "challenge-1".to_owned(),
            challenger: vec![0x11; 20],
            provider: vec![0x22; 20],
            chunk_indices: vec![0, 3],
            block_height: 42,
            merkle_root: vec![1; 32],
            seed: vec![2; 32],
            expiration_block: 142,
            timestamp: 1_700_000_000,
        },
    ));

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        let seen = right_app.seen();
        let kinds = seen.iter().filter(|(peer_id, _)| peer_id == "left").count();
        if kinds >= 3 {
            assert!(
                seen.iter()
                    .any(|(_, message)| *message == push(7, b"hello".to_vec()))
            );
            assert!(seen.iter().any(|(_, message)| *message == request(4)));
            assert!(seen.iter().any(|(_, message)| *message == challenge()));
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("started messages did not arrive: {:?}", right_app.seen());
}

#[test]
fn a_fetch_is_answered_with_the_blob() {
    let left_app = Recording::answer();
    let right_app = Recording::answer();
    let left = opened(left_app.clone());
    let right = opened(right_app.clone());
    let left_switch = attach(&left);
    let right_switch = attach(&right);
    connect(&left_switch, "right", &right_switch, "left");

    assert!(left.queue("right", &request(4)));

    let response = Message {
        sum: Some(content::message::Sum::ContentResponse(ContentResponse {
            request_id: 4,
            content_id: CONTENT_ID.to_owned(),
            blob: b"blob".to_vec(),
        })),
    };
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        if left_app
            .seen()
            .iter()
            .any(|(_, message)| *message == response)
        {
            assert!(
                left_app
                    .seen()
                    .iter()
                    .all(|(peer_id, _)| peer_id == "right")
            );
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("blob did not arrive");
}

#[test]
fn a_missing_blob_sends_no_response() {
    let left_app = Recording::answer();
    let right_app = Recording::missing();
    let left = opened(left_app.clone());
    let right = opened(right_app.clone());
    let left_switch = attach(&left);
    let right_switch = attach(&right);
    connect(&left_switch, "right", &right_switch, "left");

    assert!(left.queue("right", &request(4)));

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        if !right_app.seen().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        right_app
            .seen()
            .iter()
            .any(|(_, message)| *message == request(4)),
        "request did not arrive"
    );

    for _ in 0..20 {
        left.poll(&left_switch);
        right.poll(&right_switch);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        left_app.seen().is_empty(),
        "missing blob produced {:?}",
        left_app.seen()
    );
}

#[test]
fn a_refused_push_leaves_the_peer_connected() {
    let app = Recording::reject();
    let receiver = opened(app.clone());
    let switch = attach(&receiver);
    let sender = Switch::new();
    sender
        .add_reactor("content", channel_descriptors(), |_, _, _| {})
        .expect("sender reactor");
    connect(&switch, "sender", &sender, "recv");

    assert!(sender.send(
        "recv",
        CONTENT_CHANNEL,
        &push(1, b"no".to_vec()).encode_to_vec()
    ));

    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(500) {
        if app
            .seen()
            .iter()
            .any(|(_, message)| *message == push(1, b"no".to_vec()))
        {
            let ids: Vec<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
            assert_eq!(ids, ["sender"]);
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("push was not delivered, peers {:?}", switch.peers());
}

#[test]
fn malformed_message_stops_only_that_peer() {
    let receiver = opened(Recording::answer());
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
fn a_refusal_is_logged_and_an_oversized_reply_is_logged() {
    let logs = LogCapture::start();
    let refused = opened(Recording::reject());
    assert!(refused.handle(
        "peer-a",
        CONTENT_CHANNEL,
        &push(1, b"no".to_vec()).encode_to_vec()
    ));
    let text = logs.text();
    assert!(text.contains("app refused message"), "{text}");
    assert!(text.contains("peer=peer-a"), "{text}");
    assert!(text.contains("err=refused"), "{text}");

    let huge = opened(Recording::oversize());
    assert!(huge.handle("peer-b", CONTENT_CHANNEL, &request(4).encode_to_vec()));
    let text = logs.text();
    assert!(text.contains("reply dropped"), "{text}");
    assert!(text.contains("peer=peer-b"), "{text}");
    assert!(text.contains("queue full or message too large"), "{text}");
}

struct LogCapture(Arc<Mutex<String>>);

impl LogCapture {
    fn start() -> Self {
        let buf = Arc::new(Mutex::new(String::new()));
        set_log_capture(Some(Arc::clone(&buf)));
        Self(buf)
    }

    fn text(&self) -> String {
        self.0.lock().unwrap().clone()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        set_log_capture(None);
    }
}

#[test]
fn a_blob_over_one_mib_is_not_queued() {
    let reactor = Reactor::from_shared(Recording::answer(), Arc::new(ContentSendQueue::new()));
    let too_big = ContentPush {
        request_id: 1,
        content_id: CONTENT_ID.to_owned(),
        blob: vec![0; MAX_BLOB_BYTES + 1],
    };
    assert!(!reactor.send_queue().push("peer", &too_big));
    assert!(reactor.queue("peer", &push(1, vec![0; 4])));
}

#[test]
fn invalid_bytes_are_refused_before_the_app() {
    let app = Recording::answer();
    let reactor = opened(app.clone());
    let good = push(1, b"ok".to_vec()).encode_to_vec();

    assert!(!reactor.handle("peer", CONTENT_CHANNEL, b"\x0a"));
    assert!(!reactor.handle("peer", 0x30, &good));
    assert!(!reactor.handle(
        "peer",
        CONTENT_CHANNEL,
        &Message { sum: None }.encode_to_vec()
    ));
    assert!(!reactor.handle(
        "peer",
        CONTENT_CHANNEL,
        &push(1, vec![0; MAX_BLOB_BYTES + 1]).encode_to_vec()
    ));
    assert!(app.seen().is_empty());
}

#[test]
fn each_inbound_message_is_delivered_to_the_app() {
    let app = Recording::answer();
    let reactor = opened(app.clone());
    let messages = [
        push(7, b"hello".to_vec()),
        request(4),
        challenge(),
        ack_for(7),
        Message {
            sum: Some(content::message::Sum::ContentResponse(ContentResponse {
                request_id: 4,
                content_id: CONTENT_ID.to_owned(),
                blob: b"blob".to_vec(),
            })),
        },
        Message {
            sum: Some(content::message::Sum::CapacityChallengeResponse(
                CapacityChallengeResponse {
                    challenge_id: "challenge-1".to_owned(),
                    provider: vec![0x22; 20],
                    challenger: vec![0x11; 20],
                    block_height: 42,
                    proofs: Vec::new(),
                    generated_at: 0,
                    provider_pubkey: Vec::new(),
                    provider_signature: Vec::new(),
                },
            )),
        },
        announce(),
    ];

    for message in &messages {
        assert!(reactor.handle("peer", CONTENT_CHANNEL, &message.encode_to_vec()));
    }

    let seen = app.seen();
    assert_eq!(seen.len(), messages.len());
    for (got, want) in seen.iter().zip(messages.iter()) {
        assert_eq!(got.0, "peer");
        assert_eq!(&got.1, want);
    }
}
