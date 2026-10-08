#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `eld-tendermint start` serves genesis `status` at height 0.

use std::io::{BufRead, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine;
use eld_tendermint_abci::{read_message, write_message};
use eld_tendermint_config::MempoolConfig;
use eld_tendermint_crypto::{PrivKey, marshal_pub_key, sum};
use eld_tendermint_mempool::{App, Mempool};
use eld_tendermint_p2p::NodeKey;
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::{
    Request, RequestCheckTx, RequestInfo, Response, ResponseBeginBlock, ResponseCheckTx,
    ResponseCommit, ResponseDeliverTx, ResponseEndBlock, ResponseException, ResponseFlush,
    ResponseInfo, ResponseInitChain, ResponseQuery, request, response,
};
use eld_tendermint_types::Tx;
use serde_json::Value;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const CHAIN_ID: &str = "status-chain";

#[test]
fn status_health_and_unknown_method() {
    let home = TestHome::new("status");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();

    let (status_code, status_headers, status) =
        post(&addr, r#"{"jsonrpc":"2.0","id":7,"method":"status"}"#);
    assert_eq!(status_code, 200);
    assert!(
        status_headers
            .to_ascii_lowercase()
            .contains("content-type: application/json")
    );
    assert_eq!(status["id"], 7);
    assert_eq!(status["result"]["node_info"]["network"], CHAIN_ID);
    assert_eq!(status["result"]["sync_info"]["latest_block_height"], "0");
    assert_eq!(
        status["result"]["node_info"]["protocol_version"]["p2p"],
        "8"
    );
    assert_eq!(
        status["result"]["node_info"]["protocol_version"]["block"],
        "11"
    );
    assert_eq!(
        status["result"]["node_info"]["protocol_version"]["app"],
        "0"
    );
    assert_eq!(status["result"]["validator_info"]["voting_power"], "10");
    let address = status["result"]["validator_info"]["address"]
        .as_str()
        .expect("validator address");
    assert_eq!(address.len(), 40);
    assert_eq!(address, address.to_ascii_uppercase());
    assert_eq!(status["result"]["sync_info"]["catching_up"], false);
    assert_eq!(status["result"]["sync_info"]["latest_block_hash"], "");
    assert_eq!(
        status["result"]["sync_info"]["latest_block_time"],
        "1970-01-01T00:00:00Z"
    );
    let pv = FilePV::load(
        home.path.join("config/priv_validator_key.json"),
        home.path.join("data/priv_validator_state.json"),
    )
    .expect("priv validator");
    let pub_key = &status["result"]["validator_info"]["pub_key"];
    assert_eq!(pub_key["type"], "tendermint/PubKeyEd25519");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(pub_key["value"].as_str().expect("pubkey value"))
        .expect("pubkey bytes");
    assert_eq!(decoded, pv.get_pub_key().as_bytes().as_slice());

    let (_code, _headers, health) =
        post(&addr, r#"{"jsonrpc":"2.0","id":"abc","method":"health"}"#);
    assert_eq!(health["id"], "abc");
    assert_eq!(health["result"], serde_json::json!({}));

    let (_code, _headers, unknown) =
        post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"subscribe"}"#);
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(unknown["error"]["message"], "Method not found");
    assert!(unknown.get("result").is_none());

    let (_code, _headers, bad) = post(&addr, "not-json");
    assert_eq!(bad["error"]["code"], -32700);
}

#[test]
fn rpc_bind_failure_exits_1() {
    let home = TestHome::new("bind");
    let proxy = stub_abci();
    let hold = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = hold.local_addr().unwrap().port();
    write_home(&home.path, &proxy, &format!("tcp://127.0.0.1:{port}"));
    let mut node = NodeChild::spawn(&home.path);
    let status = node.wait_exit();
    assert_eq!(status.code(), Some(1));
    let stderr = node.stderr();
    assert!(
        stderr.to_ascii_lowercase().contains("in use"),
        "stderr: {stderr}"
    );
    drop(hold);
}

/// `params.tx` is standard base64, the amino JSON encoding of a byte slice.
#[test]
fn broadcast_tx_sync_returns_code_and_hash() {
    let home = TestHome::new("sync-ok");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let tx = b"pay";
    let (_code, _headers, body) = post(&node.rpc_addr(), &broadcast("broadcast_tx_sync", tx));
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["code"], 0);
    assert_eq!(body["result"]["hash"], hex_upper(&sum(tx)));
    assert_eq!(body["result"]["data"], "");
}

#[test]
fn broadcast_tx_sync_accepts_a_positional_tx() {
    let home = TestHome::new("sync-array");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let tx = b"pay";
    let encoded = b64(tx);
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"broadcast_tx_sync","params":["{encoded}"]}}"#
    );
    let (_code, _headers, body) = post(&node.rpc_addr(), &body);
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["code"], 0);
    assert_eq!(body["result"]["hash"], hex_upper(&sum(tx)));
}

#[test]
fn broadcast_tx_sync_returns_reject_code() {
    let home = TestHome::new("sync-reject");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let tx = b"reject";
    let (_code, _headers, body) = post(&node.rpc_addr(), &broadcast("broadcast_tx_sync", tx));
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["code"], 9);
    assert_eq!(body["result"]["log"], "rejected");
    assert_eq!(body["result"]["hash"], hex_upper(&sum(tx)));
}

#[test]
fn rejected_check_tx_is_not_pooled() {
    struct Reject;
    impl App for Reject {
        fn check_tx(&mut self, _request: RequestCheckTx) -> ResponseCheckTx {
            ResponseCheckTx {
                code: 9,
                ..ResponseCheckTx::default()
            }
        }
    }
    let mut pool = Mempool::new(MempoolConfig::test_config(), Reject).expect("pool");
    let tx = Tx::new(b"reject".as_slice());
    let response = pool.check_tx_response(&tx).expect("check");
    assert_eq!(response.code, 9);
    assert_eq!(pool.size(), 0);
}

#[test]
fn broadcast_tx_commit_returns_deliver_code() {
    let home = TestHome::new("commit");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"50ms\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = true
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let mut node = NodeChild::spawn(&home.path);
    let tx = b"pay";
    let (_code, _headers, body) = post_for(
        &node.rpc_addr(),
        &broadcast("broadcast_tx_commit", tx),
        Duration::from_secs(20),
    );
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["check_tx"]["code"], 0);
    assert_eq!(body["result"]["deliver_tx"]["code"], 7);
    assert!(body["result"]["height"].as_i64().unwrap() >= 1);
    assert_eq!(body["result"]["hash"], hex_upper(&sum(tx)));
}

#[test]
fn tx_and_tx_search_survive_restart() {
    let home = TestHome::new("tx-index");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"2s\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = true
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let tx = b"pay";
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let (_code, _headers, status) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    assert_eq!(status["result"]["node_info"]["other"]["tx_index"], "on");
    let (_code, _headers, committed) = post_for(
        &addr,
        &broadcast("broadcast_tx_commit", tx),
        Duration::from_secs(20),
    );
    assert!(committed.get("error").is_none(), "{committed}");
    let height = committed["result"]["height"].as_i64().unwrap();
    assert!(height >= 1, "{committed}");
    let hash = committed["result"]["hash"].as_str().unwrap().to_owned();
    assert_eq!(hash, hex_upper(&sum(tx)));
    let found = wait_for_tx(&addr, &hash);
    assert_eq!(found["result"]["height"], height);
    assert_eq!(found["result"]["index"], 0);
    assert_eq!(found["result"]["tx_result"]["code"], 7);
    assert_eq!(
        found["result"]["tx"],
        base64::engine::general_purpose::STANDARD.encode(tx)
    );
    assert!(found["result"].get("proof").is_none());
    drop(node);

    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let (_code, _headers, found) = post(&addr, &tx_by_hash(&hash));
    assert!(found.get("error").is_none(), "{found}");
    assert_eq!(found["result"]["hash"], hash);
    assert_eq!(found["result"]["height"], height);
    assert_eq!(found["result"]["index"], 0);

    // Height 1 is often empty: the proposer builds it before this RPC call arrives.
    let query = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"tx_search","params":{{"query":"tx.height = {height}","prove":true,"order_by":"asc"}}}}"#
    );
    let (_code, _headers, search) = post(&addr, &query);
    assert!(search.get("error").is_none(), "{search}");
    assert_eq!(search["result"]["total_count"], 1);
    assert_eq!(search["result"]["txs"][0]["hash"], hash);
    assert!(search["result"]["txs"][0].get("proof").is_none());

    let (_code, _headers, missing) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"tx","params":{"hash":"0000000000000000000000000000000000000000000000000000000000000000"}}"#,
    );
    assert_eq!(missing["error"]["code"], -32603);
    assert_eq!(
        missing["error"]["data"],
        "tx (0000000000000000000000000000000000000000000000000000000000000000) not found"
    );

    let (_code, _headers, tag) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"tx_search","params":{"query":"app.creator = 'bob'"}}"#,
    );
    assert_eq!(tag["error"]["code"], -32603);
    assert_eq!(tag["error"]["message"], "Internal error");
    assert_eq!(tag["error"]["data"], "tag app.creator is not supported");
}

fn tx_by_hash(hash: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"tx","params":{{"hash":"{hash}","prove":true}}}}"#)
}

/// `broadcast_tx_commit` returns from DeliverTx, before the index write.
fn wait_for_tx(addr: &str, hash: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_code, _headers, body) = post(addr, &tx_by_hash(hash));
        if body.get("error").is_none() {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "tx {hash} was not indexed: {body}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn broadcast_tx_commit_times_out_when_undelivered() {
    let home = TestHome::new("timeout");
    let proxy = stub_abci();
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        false,
        "timeout_broadcast_tx_commit = \"200ms\"",
        "",
        "",
    );
    let mut node = NodeChild::spawn(&home.path);
    let (_code, _headers, body) = post_for(
        &node.rpc_addr(),
        &broadcast("broadcast_tx_commit", b"pay"),
        Duration::from_secs(5),
    );
    assert!(body.get("result").is_none(), "{body}");
    assert_eq!(body["error"]["code"], -32603);
    assert_eq!(body["error"]["message"], "Internal error");
    assert_eq!(
        body["error"]["data"],
        "timed out waiting for tx to be included in a block"
    );
}

#[test]
fn abci_info_returns_the_app_response() {
    let info = ResponseInfo {
        data: "{\"size\":3}".to_owned(),
        version: "eld-app".to_owned(),
        app_version: 7,
        last_block_height: 42,
        last_block_app_hash: vec![0; 32].into(),
    };
    let info_requests = Arc::new(std::sync::Mutex::new(Vec::<RequestInfo>::new()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let saved = Arc::clone(&info_requests);
    let response_info = info.clone();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let saved = Arc::clone(&saved);
            let response_info = response_info.clone();
            thread::spawn(move || {
                serve_abci_script(
                    stream,
                    Arc::new(AtomicU64::new(0)),
                    None,
                    response_info,
                    Some(saved),
                );
            });
        }
    });
    let home = TestHome::new("abci-info");
    write_home(&home.path, &format!("tcp://{addr}"), "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let (_code, _headers, body) = post(
        &node.rpc_addr(),
        r#"{"jsonrpc":"2.0","id":1,"method":"abci_info","params":null}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["id"], 1);
    let response = &body["result"]["response"];
    assert_eq!(response["data"], "{\"size\":3}");
    assert_eq!(response["version"], "eld-app");
    assert_eq!(response["app_version"], "7");
    assert_eq!(response["last_block_height"], "42");
    assert_eq!(
        response["last_block_app_hash"],
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    );
    let requests = info_requests.lock().unwrap();
    assert_eq!(requests.len(), 2, "handshake Info plus the RPC Info");
    for req in requests.iter() {
        assert_eq!(req.version, "0.34.24");
        assert_eq!(req.block_version, 11);
        assert_eq!(req.p2p_version, 8);
    }
}

#[test]
fn abci_query_returns_the_app_value() {
    let home = TestHome::new("query");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let (_code, _headers, body) = post(
        &node.rpc_addr(),
        r#"{"jsonrpc":"2.0","id":1,"method":"abci_query","params":{"path":"/store","data":"","height":0,"prove":false}}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    let response = &body["result"]["response"];
    assert_eq!(response["code"], 0);
    assert_eq!(
        response["value"],
        base64::engine::general_purpose::STANDARD.encode(b"queried")
    );
    assert_eq!(response["key"], "");
    assert_eq!(response["height"], "0");
    assert_eq!(response["index"], "0");
    assert!(response["proofOps"].is_null());
}

#[test]
fn block_and_commit_return_height_one() {
    let home = TestHome::new("block");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"50ms\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = false
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    wait_for_height(&addr, 1);

    let (_code, _headers, block) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"block","params":{"height":1}}"#,
    );
    assert!(block.get("error").is_none(), "{block}");
    assert_eq!(block["result"]["block"]["header"]["height"], "1");
    let hash = block["result"]["block_id"]["hash"].as_str().unwrap();
    let decoded = decode_hex(hash);
    assert_eq!(decoded.len(), 32);
    let (_code, _headers, status) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    let latest = decode_hex(
        status["result"]["sync_info"]["latest_block_hash"]
            .as_str()
            .expect("latest hash"),
    );
    assert_eq!(decoded, latest);

    let (_code, _headers, commit) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"commit","params":{"height":1}}"#,
    );
    assert!(commit.get("error").is_none(), "{commit}");
    assert_eq!(commit["result"]["canonical"], false);
    assert_eq!(
        commit["result"]["signed_header"]["commit"]["block_id"]["hash"],
        hash
    );

    let (_code, _headers, missing) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"block","params":{"height":99}}"#,
    );
    assert!(missing.get("result").is_none(), "{missing}");
    assert_eq!(missing["error"]["code"], -32603);
    assert_eq!(missing["error"]["message"], "Internal error");
    let data = missing["error"]["data"].as_str().unwrap();
    assert!(
        data.starts_with("height 99 must be less than or equal to the current blockchain height "),
        "{data}"
    );
}

#[test]
fn skip_timeout_commit_false_reaches_height_two() {
    let home = TestHome::new("height-two");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"50ms\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = false
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let mut node = NodeChild::spawn(&home.path);
    wait_for_height(&node.rpc_addr(), 2);
}

#[test]
fn websocket_subscribe_new_block_and_tx() {
    let home = TestHome::new("ws");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"50ms\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = true
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let height = chain_height(&addr);

    let (mut socket, _) =
        tungstenite::connect(format!("ws://{addr}/websocket")).expect("websocket upgrade");
    socket
        .send(Message::text(
            r#"{"jsonrpc":"2.0","id":9,"method":"health"}"#,
        ))
        .expect("health");
    let health = read_matching(&mut socket, Duration::from_secs(5), |value| {
        value["id"] == 9
    });
    assert_eq!(health["error"]["code"], -32601);
    assert_eq!(health["error"]["message"], "Method not found");
    assert!(health["error"].get("data").is_none(), "{health}");

    let new_block = "tm.event = 'NewBlock'";
    socket
        .send(Message::text(rpc_call(1, "subscribe", new_block)))
        .expect("subscribe NewBlock");
    let ack = read_matching(&mut socket, Duration::from_secs(5), |value| {
        is_ack(value, 1)
    });
    assert_eq!(ack["result"], serde_json::json!({}));
    let event = read_matching(&mut socket, Duration::from_secs(20), |value| {
        header_height(value).is_some_and(|got| got > height)
    });
    assert_eq!(event["id"], 1);
    assert_eq!(event["result"]["query"], new_block);
    assert_eq!(event["result"]["data"]["canonical"], false);
    assert_eq!(event["result"]["events"]["tm.event"][0], "NewBlock");

    socket
        .send(Message::text(rpc_call(
            4,
            "subscribe",
            "tm.event = 'NewRound'",
        )))
        .expect("subscribe NewRound");
    let unsupported = read_matching(&mut socket, Duration::from_secs(5), |value| {
        value["id"] == 4
    });
    assert_eq!(unsupported["error"]["code"], -32603);
    assert_eq!(unsupported["error"]["message"], "Internal error");

    socket
        .send(Message::text(rpc_call(
            5,
            "unsubscribe",
            "tm.event = 'NoSuch'",
        )))
        .expect("unsubscribe missing");
    let missing = read_matching(&mut socket, Duration::from_secs(5), |value| {
        value["id"] == 5
    });
    assert_eq!(missing["error"]["code"], -32603);
    assert_eq!(missing["error"]["data"], "subscription not found");

    let tx_query = "tm.event = 'Tx'";
    socket
        .send(Message::text(rpc_call(2, "subscribe", tx_query)))
        .expect("subscribe Tx");
    let tx_ack = read_matching(&mut socket, Duration::from_secs(5), |value| {
        is_ack(value, 2)
    });
    assert_eq!(tx_ack["result"], serde_json::json!({}));

    let tx = b"ws-pay";
    let hash = hex_upper(&sum(tx));
    let addr_http = addr.clone();
    let body = broadcast("broadcast_tx_commit", tx);
    let http = thread::spawn(move || post_for(&addr_http, &body, Duration::from_secs(20)));
    let want = hash.clone();
    let tx_event = read_matching(&mut socket, Duration::from_secs(20), move |value| {
        value["result"]["data"]["hash"].as_str() == Some(want.as_str())
    });
    assert_eq!(tx_event["id"], 2);
    assert_eq!(tx_event["result"]["query"], tx_query);
    assert_eq!(tx_event["result"]["events"]["tm.event"][0], "Tx");
    let (_code, _headers, committed) = http.join().expect("broadcast thread");
    assert!(committed.get("error").is_none(), "{committed}");
    assert_eq!(committed["result"]["hash"], hash);

    socket
        .send(Message::text(rpc_call(3, "unsubscribe", new_block)))
        .expect("unsubscribe NewBlock");
    let unsub = read_matching(&mut socket, Duration::from_secs(5), |value| {
        is_ack(value, 3)
    });
    assert_eq!(unsub["result"], serde_json::json!({}));

    let before = chain_height(&addr);
    let start = Instant::now();
    let mut moved = false;
    while start.elapsed() < Duration::from_secs(10) {
        match read_ws(&mut socket, Duration::from_millis(200)) {
            Ok(value) => assert_no_new_block(&value),
            Err(err) if is_timeout(&err) => {}
            Err(err) => panic!("{err}"),
        }
        if chain_height(&addr) > before {
            moved = true;
            break;
        }
    }
    assert!(moved, "status height did not move after unsubscribe");
    match read_ws(&mut socket, Duration::from_millis(400)) {
        Ok(value) => assert_no_new_block(&value),
        Err(err) if is_timeout(&err) => {}
        Err(err) => panic!("{err}"),
    }

    drop(socket);
    assert!(node.still_running(), "node exited when the socket dropped");
    let (_code, _headers, status) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    assert!(status.get("error").is_none(), "{status}");
    assert_eq!(status["result"]["node_info"]["network"], CHAIN_ID);
    assert!(node.still_running(), "node exited after status");
}

fn chain_height(addr: &str) -> i64 {
    let (_code, _headers, status) = post(addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    status_height(
        &status["result"]["sync_info"]["latest_block_height"],
        &status,
    )
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn rpc_call(id: i64, method: &str, query: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": { "query": query },
    })
    .to_string()
}

fn is_ack(value: &Value, id: i64) -> bool {
    value["id"] == id && value.get("error").is_none() && value["result"].get("data").is_none()
}

fn assert_no_new_block(value: &Value) {
    assert_ne!(
        value["result"]["events"]["tm.event"][0].as_str(),
        Some("NewBlock"),
        "{value}"
    );
}

fn header_height(value: &Value) -> Option<i64> {
    value["result"]["data"]["signed_header"]["header"]["height"]
        .as_str()
        .and_then(|text| text.parse().ok())
}

fn read_matching(
    socket: &mut Socket,
    total: Duration,
    mut pred: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + total;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            panic!("timed out waiting for a websocket message");
        }
        match read_ws(socket, left.min(Duration::from_secs(2))) {
            Ok(value) if pred(&value) => return value,
            Ok(_) => {}
            Err(err) if is_timeout(&err) => {}
            Err(err) => panic!("{err}"),
        }
    }
}

fn read_ws(socket: &mut Socket, timeout: Duration) -> Result<Value, tungstenite::Error> {
    match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp.set_read_timeout(Some(timeout)).expect("timeout"),
        _ => panic!("expected a plain websocket"),
    }
    loop {
        match socket.read()? {
            Message::Text(text) => {
                let text = text.as_ref();
                return Ok(serde_json::from_str(text).unwrap_or_else(|err| panic!("{err}: {text}")));
            }
            Message::Ping(data) => {
                socket.send(Message::Pong(data))?;
            }
            Message::Close(_) => return Err(tungstenite::Error::ConnectionClosed),
            _ => {}
        }
    }
}

fn is_timeout(err: &tungstenite::Error) -> bool {
    matches!(
        err,
        tungstenite::Error::Io(io)
            if matches!(
                io.kind(),
                ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
            )
    )
}

#[test]
fn genesis_chain_id_matches_the_file() {
    let home = TestHome::new("genesis");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let file: Value = serde_json::from_slice(
        &std::fs::read(home.path.join("config/genesis.json")).expect("genesis file"),
    )
    .expect("genesis json");
    let mut node = NodeChild::spawn(&home.path);
    let (_code, _headers, body) = post(
        &node.rpc_addr(),
        r#"{"jsonrpc":"2.0","id":1,"method":"genesis"}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    let genesis = &body["result"]["genesis"];
    assert_eq!(genesis["chain_id"], file["chain_id"]);
    assert!(genesis.get("genesis_time").is_some());
    assert!(genesis.get("validators").is_some());
    assert!(genesis.get("app_hash").is_some());
    assert!(genesis.get("consensus_params").is_some());
}

#[test]
fn validators_at_height_one_are_the_genesis_set() {
    let home = TestHome::new("validators");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let file: Value = serde_json::from_slice(
        &std::fs::read(home.path.join("config/genesis.json")).expect("genesis file"),
    )
    .expect("genesis json");
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let (_code, _headers, status) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    let (_code, _headers, body) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"validators","params":{"height":1,"page":1}}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["block_height"], 1);
    assert_eq!(body["result"]["count"], 1);
    assert_eq!(body["result"]["total"], 1);
    let validator = &body["result"]["validators"][0];
    assert_eq!(
        validator["address"],
        status["result"]["validator_info"]["address"]
    );
    assert_eq!(validator["voting_power"], 10);
    assert_eq!(validator["pub_key"], file["validators"][0]["pub_key"]);
    assert_eq!(validator["pub_key"]["type"], "tendermint/PubKeyEd25519");
    assert!(validator["proposer_priority"].is_number());
}

#[test]
fn blockchain_one_through_one_returns_one_meta() {
    let home = TestHome::new("blockchain");
    let proxy = stub_abci();
    let consensus = "\
[consensus]
timeout_propose = \"50ms\"
timeout_propose_delta = \"0s\"
timeout_prevote = \"50ms\"
timeout_prevote_delta = \"0s\"
timeout_precommit = \"50ms\"
timeout_precommit_delta = \"0s\"
timeout_commit = \"50ms\"
skip_timeout_commit = true
";
    write_configured(
        &home.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        "",
        consensus,
    );
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let (_code, _headers, committed) = post_for(
        &addr,
        &broadcast("broadcast_tx_commit", b"pay"),
        Duration::from_secs(20),
    );
    assert!(committed.get("error").is_none(), "{committed}");
    let (_code, _headers, body) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"blockchain","params":{"minHeight":1,"maxHeight":1}}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    assert!(body["result"]["last_height"].as_i64().unwrap() >= 1);
    let metas = body["result"]["block_metas"].as_array().expect("metas");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0]["header"]["height"], "1");
    let meta_hash = decode_hex(metas[0]["block_id"]["hash"].as_str().unwrap());
    assert_eq!(meta_hash.len(), 32);
}

#[test]
fn net_info_counts_a_persistent_peer() {
    let proxy = stub_abci();
    let home_a = TestHome::new("net-a");
    write_home(&home_a.path, &proxy, "tcp://127.0.0.1:0");
    let mut node_a = NodeChild::spawn(&home_a.path);
    let addr_a = node_a.rpc_addr();
    let (_code, _headers, status) = post(&addr_a, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    let id = status["result"]["node_info"]["id"]
        .as_str()
        .expect("node id")
        .to_owned();
    let (_code, _headers, info) = post(&addr_a, r#"{"jsonrpc":"2.0","id":1,"method":"net_info"}"#);
    assert!(info.get("error").is_none(), "{info}");
    assert_eq!(info["result"]["listening"], true);
    assert_eq!(info["result"]["n_peers"], 0);
    let listener = info["result"]["listeners"][0]
        .as_str()
        .expect("listener")
        .to_owned();
    let host = listener
        .strip_prefix("tcp://")
        .expect("tcp listener")
        .to_owned();
    let home_b = TestHome::new("net-b");
    write_configured(
        &home_b.path,
        &proxy,
        "tcp://127.0.0.1:0",
        true,
        "",
        &format!("{id}@{host}"),
        "",
    );
    let mut node_b = NodeChild::spawn(&home_b.path);
    let addr_b = node_b.rpc_addr();
    let deadline = Instant::now() + Duration::from_secs(10);
    let body = loop {
        let (_code, _headers, body) =
            post(&addr_b, r#"{"jsonrpc":"2.0","id":1,"method":"net_info"}"#);
        if body["result"]["n_peers"] == 1 {
            break body;
        }
        assert!(Instant::now() < deadline, "peer did not connect: {body}");
        thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(body["result"]["peers"][0]["node_info"]["id"], id);
}

#[test]
fn consensus_state_on_a_fresh_home_is_height_one() {
    let home = TestHome::new("consensus-state");
    let proxy = stub_abci();
    // This key is not in the genesis set, so the round stays open at height 1.
    write_configured(&home.path, &proxy, "tcp://127.0.0.1:0", false, "", "", "");
    let mut node = NodeChild::spawn(&home.path);
    let (_code, _headers, body) = post(
        &node.rpc_addr(),
        r#"{"jsonrpc":"2.0","id":1,"method":"consensus_state"}"#,
    );
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["height"], 1);
    assert!(body["result"]["round"].is_number());
    assert!(body["result"]["step"].as_i64().unwrap() > 1);
}

#[test]
fn broadcast_tx_async_returns_before_the_next_block() {
    let home = TestHome::new("async");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let addr = node.rpc_addr();
    let (_code, _headers, before) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    assert_eq!(before["result"]["sync_info"]["latest_block_height"], "0");
    let tx = b"pay";
    let (_code, _headers, body) = post(&addr, &broadcast("broadcast_tx_async", tx));
    assert!(body.get("error").is_none(), "{body}");
    assert_eq!(body["result"]["code"], 0);
    assert_eq!(body["result"]["hash"], hex_upper(&sum(tx)));
    assert_eq!(body["result"]["data"], "");
    let (_code, _headers, after) = post(&addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
    assert_eq!(after["result"]["sync_info"]["latest_block_height"], "0");
}

fn wait_for_height(addr: &str, want: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_code, _headers, body) = post(addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
        let height = status_height(&body["result"]["sync_info"]["latest_block_height"], &body);
        if height >= want {
            return;
        }
        assert!(Instant::now() < deadline, "height stayed {height}: {body}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn broadcast(method: &str, tx: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(tx);
    format!(r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{{"tx":"{encoded}"}}}}"#)
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16)
                .unwrap_or_else(|err| panic!("{err}: {text}"))
        })
        .collect()
}

fn status_height(value: &Value, body: &Value) -> i64 {
    value
        .as_str()
        .unwrap_or_else(|| panic!("height: {body}"))
        .parse()
        .unwrap_or_else(|err| panic!("{err}: {body}"))
}

struct TestHome {
    path: PathBuf,
}

impl TestHome {
    fn new(label: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("eld-tm-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("config")).unwrap();
        std::fs::create_dir_all(path.join("data")).unwrap();
        Self { path }
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct NodeChild {
    child: Child,
    lines: Receiver<String>,
    stderr: Option<JoinHandle<String>>,
    stderr_text: Option<String>,
}

impl NodeChild {
    fn spawn(home: &Path) -> Self {
        let mut child = Command::new(bin_path())
            .arg("start")
            .arg("--home")
            .arg(home)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = std::io::BufReader::new(stdout);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                let _ = tx.send(line.clone());
                line.clear();
            }
        });
        let stderr = thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        Self {
            child,
            lines,
            stderr: Some(stderr),
            stderr_text: None,
        }
    }

    fn rpc_addr(&mut self) -> String {
        let line = match self.lines.recv_timeout(Duration::from_secs(30)) {
            Ok(line) => line,
            Err(_) => {
                let stderr = self.stderr();
                panic!("timed out waiting for the rpc address\n{stderr}");
            }
        };
        let Some(addr) = line.trim().strip_prefix("rpc: ") else {
            let stderr = self.stderr();
            panic!("expected rpc address, got {line:?}\n{stderr}");
        };
        addr.to_owned()
    }

    fn still_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn wait_exit(&mut self) -> std::process::ExitStatus {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if start.elapsed() > Duration::from_secs(30) {
                let stderr = self.stderr();
                panic!("node did not exit\n{stderr}");
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn stderr(&mut self) -> String {
        if self.stderr_text.is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(handle) = self.stderr.take() {
                self.stderr_text = Some(handle.join().unwrap_or_default());
            }
        }
        self.stderr_text.clone().unwrap_or_default()
    }
}

impl Drop for NodeChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(handle) = self.stderr.take() {
            let _ = handle.join();
        }
    }
}

fn write_home(home: &Path, proxy_app: &str, rpc_laddr: &str) {
    write_configured(home, proxy_app, rpc_laddr, true, "", "", "");
}

fn write_configured(
    home: &Path,
    proxy_app: &str,
    rpc_laddr: &str,
    own_validator: bool,
    rpc_extra: &str,
    persistent_peers: &str,
    consensus: &str,
) {
    let pv = FilePV::load_or_gen_file_pv(
        home.join("config/priv_validator_key.json"),
        home.join("data/priv_validator_state.json"),
    )
    .unwrap();
    NodeKey::load_or_gen(home.join("config/node_key.json")).unwrap();
    let own_key: Value = serde_json::from_str(&marshal_pub_key(&pv.get_pub_key())).unwrap();
    let pub_key = if own_validator {
        own_key
    } else {
        let other = PrivKey::generate().public_key().expect("ed25519 key");
        serde_json::from_str(&marshal_pub_key(&other)).unwrap()
    };
    let genesis = serde_json::json!({
        "genesis_time": "2019-10-13T16:14:44Z",
        "chain_id": CHAIN_ID,
        "initial_height": "1",
        "validators": [{
            "pub_key": pub_key,
            "power": "10",
            "name": ""
        }]
    });
    std::fs::write(
        home.join("config/genesis.json"),
        serde_json::to_vec(&genesis).unwrap(),
    )
    .unwrap();
    let config = format!(
        "proxy_app = {proxy_app:?}\nmoniker = \"status-node\"\n\n[rpc]\nladdr = {rpc_laddr:?}\n{rpc_extra}\n\n[p2p]\nladdr = \"tcp://127.0.0.1:0\"\npersistent_peers = {persistent_peers:?}\n\n{consensus}"
    );
    std::fs::write(home.join("config/config.toml"), config).unwrap();
}

#[test]
fn refused_privval_dial_stops_startup() {
    let home = TestHome::new("privval-dial");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let config_path = home.path.join("config/config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(
        &config_path,
        format!("priv_validator_laddr = \"127.0.0.1:1\"\n{config}"),
    )
    .unwrap();
    std::fs::remove_file(home.path.join("config/priv_validator_key.json")).unwrap();

    let mut node = NodeChild::spawn(&home.path);
    let status = node.wait_exit();
    assert!(!status.success(), "startup continued after a refused dial");
    let stderr = node.stderr();
    assert!(stderr.contains("127.0.0.1:1"), "{stderr}");
    assert!(!stderr.contains("priv_validator_key.json"), "{stderr}");
}

#[test]
fn fresh_start_logs_handshake_and_init_chain_once() {
    let home = TestHome::new("operator-logs");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");

    let mut node = NodeChild::spawn(&home.path);
    let _addr = node.rpc_addr();
    let stderr = node.stderr();
    assert!(stderr.contains("ABCI Handshake"), "{stderr}");
    assert!(stderr.contains("InitChain"), "{stderr}");
    assert!(stderr.contains(&format!("chain_id={CHAIN_ID}")), "{stderr}");

    let mut again = NodeChild::spawn(&home.path);
    let _addr = again.rpc_addr();
    let stderr = again.stderr();
    assert!(stderr.contains("ABCI Handshake"), "{stderr}");
    assert!(!stderr.contains("InitChain"), "{stderr}");
}

#[test]
fn refused_proxy_app_retries_without_exiting() {
    let home = TestHome::new("proxy-refused");
    write_home(&home.path, "tcp://127.0.0.1:1", "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(1) {
        assert!(
            node.still_running(),
            "node exited while the ABCI app was down"
        );
        thread::sleep(Duration::from_millis(50));
    }
    let stderr = node.stderr();
    assert!(
        stderr.contains(
            "abci.socketClient failed to connect to tcp://127.0.0.1:1. Retrying after 3s..."
        ),
        "{stderr}"
    );
}

#[test]
fn proxy_app_dial_succeeds_after_retry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let home = TestHome::new("proxy-retry");
    write_home(&home.path, &format!("tcp://{addr}"), "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    thread::sleep(Duration::from_secs(1));
    assert!(node.still_running(), "node exited before the app listened");
    let listener = TcpListener::bind(addr).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let calls = Arc::new(AtomicU64::new(0));
            thread::spawn(move || serve_abci(stream, calls, None));
        }
    });
    let _addr = node.rpc_addr();
    let stderr = node.stderr();
    assert!(
        stderr.contains("abci.socketClient failed to connect to"),
        "{stderr}"
    );
    assert!(stderr.contains("ABCI Handshake"), "{stderr}");
}

#[test]
fn info_and_init_chain_use_different_sockets() {
    let conns = Arc::new(std::sync::Mutex::new(Vec::<Arc<OpenSocket>>::new()));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let init_chain_calls = Arc::new(AtomicU64::new(0));
    let accepted = Arc::clone(&conns);
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let seen = Arc::new(OpenSocket {
                requests: Arc::new(std::sync::Mutex::new(Vec::new())),
            });
            accepted.lock().unwrap().push(Arc::clone(&seen));
            let init_chain_calls = Arc::clone(&init_chain_calls);
            thread::spawn(move || {
                serve_abci(stream, init_chain_calls, Some(Arc::clone(&seen.requests)));
            });
        }
    });
    let home = TestHome::new("abci-sockets");
    write_home(&home.path, &format!("tcp://{addr}"), "tcp://127.0.0.1:0");
    let mut node = NodeChild::spawn(&home.path);
    let _addr = node.rpc_addr();
    let conns = conns.lock().unwrap();
    assert_eq!(
        conns.len(),
        4,
        "Go opens query, snapshot, mempool, and consensus"
    );
    let mut saw_info = false;
    let mut saw_init = false;
    for conn in conns.iter() {
        let requests = conn.requests.lock().unwrap();
        let info = requests.contains(&"info");
        let init = requests.contains(&"init_chain");
        assert!(
            !(info && init),
            "Info and InitChain share a socket: {requests:?}"
        );
        saw_info |= info;
        saw_init |= init;
    }
    assert!(saw_info, "query socket did not receive Info");
    assert!(saw_init, "consensus socket did not receive InitChain");
}

#[test]
fn unsafe_reset_all_wipes_chain_data_and_keeps_keys() {
    let home = TestHome::new("reset-wipe");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let key = std::fs::read(home.path.join("config/priv_validator_key.json")).unwrap();
    let genesis = std::fs::read(home.path.join("config/genesis.json")).unwrap();
    let config = std::fs::read(home.path.join("config/config.toml")).unwrap();
    let node_key = std::fs::read(home.path.join("config/node_key.json")).unwrap();

    let blockstore = home.path.join("data/blockstore.db");
    std::fs::create_dir_all(&blockstore).unwrap();
    std::fs::write(blockstore.join("CURRENT"), b"MANIFEST-000001\n").unwrap();
    std::fs::write(blockstore.join("LOG"), b"goleveldb log\n").unwrap();
    std::fs::write(blockstore.join("000001.ldb"), b"not opened").unwrap();
    std::fs::write(home.path.join("data/cs.wal"), b"wal bytes").unwrap();
    std::fs::write(
        home.path.join("config/addrbook.json"),
        b"{\"key\":\"addr\"}\n",
    )
    .unwrap();

    run_reset(&home.path, &["--home", home.path.to_str().unwrap()]);

    assert!(!blockstore.exists());
    assert!(!home.path.join("data/cs.wal").exists());
    assert!(!home.path.join("config/addrbook.json").exists());
    assert_eq!(data_entries(&home.path), vec!["priv_validator_state.json"]);
    assert_eq!(
        std::fs::read(home.path.join("config/priv_validator_key.json")).unwrap(),
        key
    );
    assert_eq!(
        std::fs::read(home.path.join("config/genesis.json")).unwrap(),
        genesis
    );
    assert_eq!(
        std::fs::read(home.path.join("config/config.toml")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read(home.path.join("config/node_key.json")).unwrap(),
        node_key
    );
    assert_height_zero_state(&home.path);
}

#[test]
fn unsafe_reset_all_keep_addr_book_leaves_the_file() {
    let home = TestHome::new("reset-keep");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let addrbook = b"{\"key\":\"keep\"}\n";
    std::fs::write(home.path.join("config/addrbook.json"), addrbook).unwrap();

    let home_arg = home.path.to_str().unwrap();
    run_reset(&home.path, &["--keep-addr-book", "--home", home_arg]);

    assert_eq!(
        std::fs::read(home.path.join("config/addrbook.json")).unwrap(),
        addrbook
    );
    assert_height_zero_state(&home.path);
}

#[test]
fn unsafe_reset_all_missing_data_and_addrbook_still_writes_state() {
    let home = TestHome::new("reset-missing");
    let proxy = stub_abci();
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    std::fs::remove_dir_all(home.path.join("data")).unwrap();
    assert!(!home.path.join("config/addrbook.json").exists());

    run_reset(&home.path, &["--home", home.path.to_str().unwrap()]);

    assert!(home.path.join("data").is_dir());
    assert_eq!(data_entries(&home.path), vec!["priv_validator_state.json"]);
    assert_height_zero_state(&home.path);
}

#[test]
fn start_after_unsafe_reset_all_calls_init_chain_once() {
    let home = TestHome::new("reset-init");
    let init_chain_calls = Arc::new(AtomicU64::new(0));
    let proxy = stub_abci_counting(Arc::clone(&init_chain_calls));
    write_home(&home.path, &proxy, "tcp://127.0.0.1:0");
    let blockstore = home.path.join("data/blockstore.db");
    std::fs::create_dir_all(&blockstore).unwrap();
    std::fs::write(blockstore.join("CURRENT"), b"MANIFEST-000001\n").unwrap();
    std::fs::write(blockstore.join("LOG"), b"goleveldb log\n").unwrap();
    std::fs::write(blockstore.join("000001.ldb"), b"not opened").unwrap();
    std::fs::write(home.path.join("data/cs.wal"), b"wal bytes").unwrap();

    run_reset(&home.path, &["--home", home.path.to_str().unwrap()]);
    assert!(!blockstore.exists());

    let mut node = NodeChild::spawn(&home.path);
    let _addr = node.rpc_addr();
    assert_eq!(init_chain_calls.load(Ordering::SeqCst), 1);
}

fn run_reset(home: &Path, args: &[&str]) {
    let output = Command::new(bin_path())
        .arg("unsafe-reset-all")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "reset of {} failed: {}",
        home.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn data_entries(home: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home.join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn assert_height_zero_state(home: &Path) {
    let pv = FilePV::load(
        home.join("config/priv_validator_key.json"),
        home.join("data/priv_validator_state.json"),
    )
    .expect("priv validator");
    assert_eq!(pv.last_sign_state.height, 0);
    assert_eq!(pv.last_sign_state.round, 0);
    assert_eq!(pv.last_sign_state.step, 0);
    assert!(pv.last_sign_state.signature.is_none());
    assert!(pv.last_sign_state.sign_bytes.is_none());
}

fn stub_abci() -> String {
    stub_abci_counting(Arc::new(AtomicU64::new(0)))
}

fn stub_abci_counting(init_chain_calls: Arc<AtomicU64>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let init_chain_calls = Arc::clone(&init_chain_calls);
            thread::spawn(move || serve_abci(stream, init_chain_calls, None));
        }
    });
    format!("tcp://{addr}")
}

struct OpenSocket {
    requests: Arc<std::sync::Mutex<Vec<&'static str>>>,
}

fn serve_abci(
    stream: TcpStream,
    init_chain_calls: Arc<AtomicU64>,
    seen: Option<Arc<std::sync::Mutex<Vec<&'static str>>>>,
) {
    serve_abci_script(
        stream,
        init_chain_calls,
        seen,
        ResponseInfo::default(),
        None,
    );
}

fn serve_abci_script(
    mut stream: TcpStream,
    init_chain_calls: Arc<AtomicU64>,
    seen: Option<Arc<std::sync::Mutex<Vec<&'static str>>>>,
    info_response: ResponseInfo,
    info_requests: Option<Arc<std::sync::Mutex<Vec<RequestInfo>>>>,
) {
    loop {
        let req: Request = match read_message(&mut stream) {
            Ok(req) => req,
            Err(_) => return,
        };
        let (name, value) = match req.value {
            Some(request::Value::Info(req)) => {
                if let Some(saved) = &info_requests {
                    saved.lock().unwrap().push(req);
                }
                ("info", response::Value::Info(info_response.clone()))
            }
            Some(request::Value::InitChain(_)) => {
                init_chain_calls.fetch_add(1, Ordering::SeqCst);
                (
                    "init_chain",
                    response::Value::InitChain(ResponseInitChain::default()),
                )
            }
            Some(request::Value::Flush(_)) => ("flush", response::Value::Flush(ResponseFlush {})),
            Some(request::Value::CheckTx(req)) => {
                let code = if req.tx.as_ref() == b"reject" { 9 } else { 0 };
                (
                    "check_tx",
                    response::Value::CheckTx(ResponseCheckTx {
                        code,
                        log: if code == 0 {
                            String::new()
                        } else {
                            "rejected".to_owned()
                        },
                        ..ResponseCheckTx::default()
                    }),
                )
            }
            Some(request::Value::BeginBlock(_)) => (
                "begin_block",
                response::Value::BeginBlock(ResponseBeginBlock::default()),
            ),
            Some(request::Value::DeliverTx(_)) => (
                "deliver_tx",
                response::Value::DeliverTx(ResponseDeliverTx {
                    code: 7,
                    ..ResponseDeliverTx::default()
                }),
            ),
            Some(request::Value::EndBlock(_)) => (
                "end_block",
                response::Value::EndBlock(ResponseEndBlock::default()),
            ),
            Some(request::Value::Commit(_)) => {
                ("commit", response::Value::Commit(ResponseCommit::default()))
            }
            Some(request::Value::Query(_)) => (
                "query",
                response::Value::Query(ResponseQuery {
                    value: b"queried".as_slice().into(),
                    ..ResponseQuery::default()
                }),
            ),
            _ => (
                "other",
                response::Value::Exception(ResponseException {
                    error: "unsupported".to_owned(),
                }),
            ),
        };
        if let Some(seen) = &seen {
            seen.lock().unwrap().push(name);
        }
        if write_message(&mut stream, &Response { value: Some(value) }).is_err() {
            return;
        }
    }
}

fn bin_path() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_eld_tendermint") {
        return PathBuf::from(path);
    }
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target");
    path.push("debug");
    path.push("eld-tendermint");
    path
}

fn post(addr: &str, body: &str) -> (u16, String, Value) {
    post_for(addr, body, Duration::from_secs(5))
}

fn post_for(addr: &str, body: &str, timeout: Duration) -> (u16, String, Value) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(timeout)).unwrap();
    let req = format!(
        "POST / HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let text = String::from_utf8(buf).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let json = serde_json::from_str(body).unwrap();
    (status, head.to_owned(), json)
}
