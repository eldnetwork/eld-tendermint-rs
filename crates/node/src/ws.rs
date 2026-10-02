//! `GET /websocket` subscriptions for `NewBlock` and `Tx`.
//!
//! One thread owns each socket. Consensus and fast sync publish into a bounded
//! channel. A full or closed channel drops that subscriber.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use eld_tendermint_proto::abci::ResponseDeliverTx;
use eld_tendermint_state::CommitEvents;
use eld_tendermint_types::{Block, Commit};
use serde_json::Value;
use tungstenite::protocol::{Role, WebSocket};
use tungstenite::{Error, Message};

const CHANNEL_BOUND: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    NewBlock,
    Tx,
}

struct Sub {
    query: String,
    id: Value,
    kind: Kind,
}

struct Outgoing {
    query: String,
    text: String,
}

struct Conn {
    tx: SyncSender<Outgoing>,
    subs: Vec<Sub>,
}

struct Inner {
    next: u64,
    conns: HashMap<u64, Conn>,
}

/// Connections and the queries each one holds.
pub struct SubscriptionHub {
    inner: Mutex<Inner>,
}

impl SubscriptionHub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                next: 1,
                conns: HashMap::new(),
            }),
        }
    }

    fn connect(&self) -> (u64, Receiver<Outgoing>) {
        let (tx, rx) = mpsc::sync_channel(CHANNEL_BOUND);
        let mut inner = lock(&self.inner);
        let id = inner.next;
        inner.next = inner.next.wrapping_add(1);
        inner.conns.insert(
            id,
            Conn {
                tx,
                subs: Vec::new(),
            },
        );
        (id, rx)
    }

    fn disconnect(&self, id: u64) {
        lock(&self.inner).conns.remove(&id);
    }

    fn holds(&self, id: u64, query: &str) -> bool {
        lock(&self.inner)
            .conns
            .get(&id)
            .is_some_and(|conn| conn.subs.iter().any(|sub| sub.query == query))
    }

    fn subscribe(&self, id: u64, query: &str, request_id: Value) -> Result<(), &'static str> {
        let Some(kind) = event_kind(query) else {
            return Err("query is not supported");
        };
        let mut inner = lock(&self.inner);
        let Some(conn) = inner.conns.get_mut(&id) else {
            return Err("subscription not found");
        };
        if conn.subs.iter().any(|sub| sub.query == query) {
            return Err("already subscribed");
        }
        conn.subs.push(Sub {
            query: query.to_owned(),
            id: request_id,
            kind,
        });
        Ok(())
    }

    fn unsubscribe(&self, id: u64, query: &str) -> Result<(), &'static str> {
        let mut inner = lock(&self.inner);
        let Some(conn) = inner.conns.get_mut(&id) else {
            return Err("subscription not found");
        };
        let before = conn.subs.len();
        conn.subs.retain(|sub| sub.query != query);
        if conn.subs.len() == before {
            Err("subscription not found")
        } else {
            Ok(())
        }
    }

    fn publish(&self, kind: Kind, data: &Value) {
        let mut inner = lock(&self.inner);
        let mut drop_ids = Vec::new();
        for (id, conn) in &inner.conns {
            for sub in &conn.subs {
                if sub.kind != kind {
                    continue;
                }
                let outgoing = Outgoing {
                    query: sub.query.clone(),
                    text: event_json(&sub.id, &sub.query, kind, data),
                };
                if conn.tx.try_send(outgoing).is_err() {
                    drop_ids.push(*id);
                    break;
                }
            }
        }
        for id in drop_ids {
            inner.conns.remove(&id);
        }
    }
}

/// Forwards each committed block to [`SubscriptionHub`].
pub struct CommitPublisher {
    hub: Arc<SubscriptionHub>,
}

impl CommitPublisher {
    #[must_use]
    pub fn new(hub: Arc<SubscriptionHub>) -> Self {
        Self { hub }
    }
}

impl CommitEvents for CommitPublisher {
    fn on_commit(&self, block: &Block, seen: &Commit, deliver_txs: &[ResponseDeliverTx]) {
        let block_data = crate::rpc::new_block_data(block, seen);
        self.hub.publish(Kind::NewBlock, &block_data);
        for (index, tx) in block.data.as_slice().iter().enumerate() {
            let Some(deliver) = deliver_txs.get(index) else {
                break;
            };
            let Some(data) =
                crate::rpc::tx_event_data(block.header.height, index, tx.as_bytes(), deliver)
            else {
                continue;
            };
            self.hub.publish(Kind::Tx, &data);
        }
    }
}

/// Read JSON-RPC text frames until the peer closes the socket.
pub(crate) fn run(stream: TcpStream, hub: &SubscriptionHub) {
    let mut socket = WebSocket::from_raw_socket(stream, Role::Server, None);
    let (id, events) = hub.connect();
    loop {
        if !read_frames(&mut socket, id, hub) {
            break;
        }
        if !write_events(&mut socket, id, hub, &events) {
            break;
        }
        if let Err(err) = socket.flush() {
            if !would_block(&err) {
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    hub.disconnect(id);
}

fn read_frames(socket: &mut WebSocket<TcpStream>, id: u64, hub: &SubscriptionHub) -> bool {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let reply = on_text(text.as_ref(), id, hub);
                if !send_text(socket, &reply) {
                    return false;
                }
            }
            Ok(Message::Close(_)) => return false,
            Ok(_) => {}
            Err(err) if would_block(&err) => return true,
            Err(_) => return false,
        }
    }
}

fn write_events(
    socket: &mut WebSocket<TcpStream>,
    id: u64,
    hub: &SubscriptionHub,
    events: &Receiver<Outgoing>,
) -> bool {
    loop {
        match events.try_recv() {
            Ok(outgoing) => {
                if !hub.holds(id, &outgoing.query) {
                    continue;
                }
                if !send_text(socket, &outgoing.text) {
                    return false;
                }
            }
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => return false,
        }
    }
}

fn send_text(socket: &mut WebSocket<TcpStream>, text: &str) -> bool {
    match socket.send(Message::text(text)) {
        Ok(()) => true,
        Err(err) if would_block(&err) => {
            let _ = socket.flush();
            true
        }
        Err(_) => false,
    }
}

fn would_block(err: &Error) -> bool {
    matches!(err, Error::Io(io) if io.kind() == ErrorKind::WouldBlock)
}

fn on_text(text: &str, id: u64, hub: &SubscriptionHub) -> String {
    let request: Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(_) => return rpc_error(Value::Null, -32700, "Parse error. Invalid JSON", None),
    };
    let request_id = echo_id(&request);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "subscribe" => subscribe(&request_id, &request, id, hub),
        "unsubscribe" => unsubscribe(&request_id, &request, id, hub),
        _ => rpc_error(request_id, -32601, "Method not found", None),
    }
}

fn subscribe(id: &Value, request: &Value, conn: u64, hub: &SubscriptionHub) -> String {
    let query = match query_param(request) {
        Ok(query) => query,
        Err(data) => return rpc_error(id.clone(), -32602, "Invalid params", Some(&data)),
    };
    match hub.subscribe(conn, query.trim(), id.clone()) {
        Ok(()) => rpc_result(id),
        Err(data) => rpc_error(id.clone(), -32603, "Internal error", Some(data)),
    }
}

fn unsubscribe(id: &Value, request: &Value, conn: u64, hub: &SubscriptionHub) -> String {
    let query = match query_param(request) {
        Ok(query) => query,
        Err(data) => return rpc_error(id.clone(), -32602, "Invalid params", Some(&data)),
    };
    match hub.unsubscribe(conn, query.trim()) {
        Ok(()) => rpc_result(id),
        Err(data) => rpc_error(id.clone(), -32603, "Internal error", Some(data)),
    }
}

fn query_param(request: &Value) -> Result<String, String> {
    let Some(params) = request.get("params") else {
        return Ok(String::new());
    };
    if params.is_null() {
        return Ok(String::new());
    }
    if !params.is_object() {
        return Err("params must be an object".to_owned());
    }
    match params.get("query") {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(_) => Err("params.query is not a string".to_owned()),
    }
}

/// `tm.event = 'NewBlock'` or `tm.event = 'Tx'`. Spaces around `=` are optional.
fn event_kind(query: &str) -> Option<Kind> {
    let (tag, value) = query.split_once('=')?;
    if tag.trim() != "tm.event" {
        return None;
    }
    let value = value.trim().strip_prefix('\'')?.strip_suffix('\'')?;
    match value {
        "NewBlock" => Some(Kind::NewBlock),
        "Tx" => Some(Kind::Tx),
        _ => None,
    }
}

fn event_json(id: &Value, query: &str, kind: Kind, data: &Value) -> String {
    let name = match kind {
        Kind::NewBlock => "NewBlock",
        Kind::Tx => "Tx",
    };
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "query": query,
            "data": data,
            "events": { "tm.event": [name] },
        },
    })
    .to_string()
}

fn echo_id(request: &Value) -> Value {
    match request.get("id") {
        Some(Value::Number(number)) => Value::Number(number.clone()),
        Some(Value::String(text)) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

fn rpc_result(id: &Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {},
    })
    .to_string()
}

fn rpc_error(id: Value, code: i64, message: &str, data: Option<&str>) -> String {
    let mut error = serde_json::json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = Value::String(data.to_owned());
    }
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": error,
    })
    .to_string()
}

fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|err| err.into_inner())
}
