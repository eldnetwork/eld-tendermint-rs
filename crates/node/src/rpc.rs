//! `POST /` JSON-RPC for `status` and `health`.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use eld_tendermint_store::{BlockStore, RocksDb};
use eld_tendermint_types::Time;
use serde_json::{Map, Value};

use crate::error::{Error, fail};

/// Fields `status` reads. Height comes from the RocksDB block store.
pub struct NodeStatus {
    pub id: String,
    pub network: String,
    pub listen_addr: String,
    pub moniker: String,
    pub rpc_address: String,
    pub channels: String,
    pub address: String,
    pub pub_key: Value,
    pub voting_power: i64,
    pub block_store: Arc<BlockStore<RocksDb>>,
}

/// Accept JSON-RPC calls until the listener closes.
///
/// # Errors
///
/// Returns an error when a response cannot be encoded. A bad client does not
/// stop the loop.
pub fn serve(listener: TcpListener, status: Arc<NodeStatus>) -> Result<(), Error> {
    for incoming in listener.incoming() {
        let Ok(mut stream) = incoming else {
            continue;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let _ = handle(&mut stream, &status);
    }
    Ok(())
}

fn handle(stream: &mut impl ReadWrite, status: &NodeStatus) -> Result<(), Error> {
    let (request_line, _headers, body) = read_request(stream)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    if method != "POST" || path != "/" {
        write_raw(stream, 404, "Not Found", b"")?;
        return Ok(());
    }
    let response = match serde_json::from_slice::<Value>(&body) {
        Ok(request) => dispatch(&request, status),
        Err(_) => rpc_error(Value::Null, -32700, "Parse error. Invalid JSON"),
    };
    let bytes = serde_json::to_vec(&response).map_err(fail)?;
    write_raw(stream, 200, "OK", &bytes)
}

fn dispatch(request: &Value, status: &NodeStatus) -> Value {
    let id = echo_id(request);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    match method {
        "health" => rpc_result(id, Value::Object(Map::new())),
        "status" => rpc_result(id, status_result(status)),
        _ => rpc_error(id, -32601, "Method not found"),
    }
}

fn status_result(status: &NodeStatus) -> Value {
    let height = status.block_store.height();
    let (latest_hash, latest_app, latest_time) = block_at(&status.block_store, height);
    let earliest_height = if height == 0 {
        0
    } else {
        status.block_store.base()
    };
    let (earliest_hash, earliest_app, earliest_time) =
        block_at(&status.block_store, earliest_height);
    serde_json::json!({
        "node_info": {
            "protocol_version": { "p2p": 8, "block": 11, "app": 0 },
            "id": status.id,
            "listen_addr": status.listen_addr,
            "network": status.network,
            "version": "0.34.24",
            "channels": status.channels,
            "moniker": status.moniker,
            "other": {
                "tx_index": "off",
                "rpc_address": status.rpc_address,
            },
        },
        "sync_info": {
            "latest_block_hash": latest_hash,
            "latest_app_hash": latest_app,
            "latest_block_height": height,
            "latest_block_time": latest_time,
            "earliest_block_hash": earliest_hash,
            "earliest_app_hash": earliest_app,
            "earliest_block_height": earliest_height,
            "earliest_block_time": earliest_time,
            "catching_up": false,
        },
        "validator_info": {
            "address": status.address,
            "pub_key": status.pub_key,
            "voting_power": status.voting_power,
        },
    })
}

fn block_at(store: &BlockStore<RocksDb>, height: i64) -> (String, String, String) {
    let epoch = Time::from_unix_parts(0, 0).to_rfc3339();
    if height == 0 {
        return (String::new(), String::new(), epoch);
    }
    let Some(meta) = store.load_block_meta(height) else {
        return (String::new(), String::new(), epoch);
    };
    (
        hex_upper(&meta.block_id.hash),
        hex_upper(&meta.header.app_hash),
        meta.header.time.to_rfc3339(),
    )
}

fn hex_upper(bytes: &[u8]) -> String {
    hex::encode(bytes).to_ascii_uppercase()
}

fn echo_id(request: &Value) -> Value {
    match request.get("id") {
        Some(Value::Number(number)) => Value::Number(number.clone()),
        Some(Value::String(text)) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

fn rpc_result(id: Value, result: Value) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

fn read_request(stream: &mut impl Read) -> Result<(String, Vec<String>, Vec<u8>), Error> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(fail)?;
        if n == 0 {
            return Err(Error::new("client closed the request"));
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 64 * 1024 {
            return Err(Error::new("request headers are too large"));
        }
    }
    let text = String::from_utf8(buf).map_err(|_| Error::new("request headers are not utf-8"))?;
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0usize;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        headers.push(line.to_owned());
    }
    if content_length > 1024 * 1024 {
        return Err(Error::new("request body is too large"));
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).map_err(fail)?;
    }
    Ok((request_line, headers, body))
}

fn write_raw(stream: &mut impl Write, status: u16, reason: &str, body: &[u8]) -> Result<(), Error> {
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).map_err(fail)?;
    stream.write_all(body).map_err(fail)?;
    stream.flush().map_err(fail)
}

trait ReadWrite: Read + Write {}

impl<T: Read + Write> ReadWrite for T {}
