//! One listener for the REST API and the JSON-RPC API.
//!
//! `OPTIONS` returns 200 with the CORS headers before either API. `GET` goes to
//! `rest`. `POST` goes to `rpc`. `GET /websocket` upgrades the socket.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::error::{Error, fail};
use crate::rpc::{self, NodeStatus};

const CORS_ALLOW_ORIGIN: &str = "https://explorer.eld.network";
const CORS_ALLOW_METHODS: &str = "GET, POST, OPTIONS";
const CORS_ALLOW_HEADERS: &str = "Content-Type";

/// Accept connections until the listener closes. Each one is REST or JSON-RPC.
///
/// # Errors
///
/// Returns an error when a response cannot be encoded. A bad client does not
/// stop the loop.
pub fn serve(listener: TcpListener, status: Arc<NodeStatus>) -> Result<(), Error> {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else {
            continue;
        };
        let status = Arc::clone(&status);
        thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
            let _ = handle(stream, &status);
        });
    }
    Ok(())
}

fn handle(mut stream: TcpStream, status: &NodeStatus) -> Result<(), Error> {
    let (request_line, headers, body) = read_request(&mut stream)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    match method {
        "OPTIONS" => write_raw(&mut stream, 200, "OK", b""),
        "GET" => {
            if path == "/websocket" {
                if upgrade_websocket(&mut stream, &headers)? {
                    let _ = stream.set_read_timeout(None);
                    let _ = stream.set_write_timeout(None);
                    let _ = stream.set_nonblocking(true);
                    crate::ws::run(stream, &status.subscriptions);
                }
                return Ok(());
            }
            match crate::rest::handle(path, status) {
                Some(response) => write_json(&mut stream, &response),
                None => write_raw(&mut stream, 404, "Not Found", b""),
            }
        }
        "POST" => {
            if path != "/" {
                return write_raw(&mut stream, 404, "Not Found", b"");
            }
            write_json(&mut stream, &rpc::handle(&body, status))
        }
        _ => write_raw(&mut stream, 404, "Not Found", b""),
    }
}

fn write_json(stream: &mut TcpStream, response: &Value) -> Result<(), Error> {
    let bytes = serde_json::to_vec(response).map_err(fail)?;
    write_raw(stream, 200, "OK", &bytes)
}

/// `true` when the stream is a WebSocket. A missing key is HTTP 400 and `false`.
fn upgrade_websocket(stream: &mut TcpStream, headers: &[String]) -> Result<bool, Error> {
    let Some(key) = header_value(headers, "sec-websocket-key") else {
        write_raw(stream, 400, "Bad Request", b"")?;
        return Ok(false);
    };
    let accept = tungstenite::handshake::derive_accept_key(key.as_bytes());
    let raw = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         \r\n"
    );
    stream.write_all(raw.as_bytes()).map_err(fail)?;
    stream.flush().map_err(fail)?;
    Ok(true)
}

fn header_value<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then_some(value.trim())
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
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: {CORS_ALLOW_ORIGIN}\r\n\
         Access-Control-Allow-Methods: {CORS_ALLOW_METHODS}\r\n\
         Access-Control-Allow-Headers: {CORS_ALLOW_HEADERS}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes()).map_err(fail)?;
    stream.write_all(body).map_err(fail)?;
    stream.flush().map_err(fail)
}
