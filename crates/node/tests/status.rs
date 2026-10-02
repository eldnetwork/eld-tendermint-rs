//! `eld-tendermint start` serves genesis `status` at height 0.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    Request, RequestCheckTx, Response, ResponseBeginBlock, ResponseCheckTx, ResponseCommit,
    ResponseDeliverTx, ResponseEndBlock, ResponseException, ResponseFlush, ResponseInfo,
    ResponseInitChain, ResponseQuery, request, response,
};
use eld_tendermint_types::Tx;
use serde_json::Value;

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
    assert_eq!(status["result"]["sync_info"]["latest_block_height"], 0);
    assert_eq!(status["result"]["sync_info"]["catching_up"], false);
    assert_eq!(status["result"]["sync_info"]["latest_block_hash"], "");
    assert_eq!(
        status["result"]["sync_info"]["latest_block_time"],
        "1970-01-01T00:00:00Z"
    );

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
    write_configured(&home.path, &proxy, "tcp://127.0.0.1:0", true, "", consensus);
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
    write_configured(&home.path, &proxy, "tcp://127.0.0.1:0", true, "", consensus);
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
    write_configured(&home.path, &proxy, "tcp://127.0.0.1:0", true, "", consensus);
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
    assert_eq!(hash.len(), 64);
    assert!(
        hash.chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_lowercase())
    );

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

fn wait_for_height(addr: &str, want: i64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (_code, _headers, body) = post(addr, r#"{"jsonrpc":"2.0","id":1,"method":"status"}"#);
        let height = body["result"]["sync_info"]["latest_block_height"]
            .as_i64()
            .unwrap_or(0);
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

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
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
    write_configured(home, proxy_app, rpc_laddr, true, "", "");
}

fn write_configured(
    home: &Path,
    proxy_app: &str,
    rpc_laddr: &str,
    own_validator: bool,
    rpc_extra: &str,
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
        "proxy_app = {proxy_app:?}\nmoniker = \"status-node\"\n\n[rpc]\nladdr = {rpc_laddr:?}\n{rpc_extra}\n\n[p2p]\nladdr = \"tcp://127.0.0.1:0\"\npersistent_peers = \"\"\n\n{consensus}"
    );
    std::fs::write(home.join("config/config.toml"), config).unwrap();
}

fn stub_abci() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || serve_abci(stream));
        }
    });
    format!("tcp://{addr}")
}

fn serve_abci(mut stream: TcpStream) {
    loop {
        let req: Request = match read_message(&mut stream) {
            Ok(req) => req,
            Err(_) => return,
        };
        let value = match req.value {
            Some(request::Value::Info(_)) => response::Value::Info(ResponseInfo::default()),
            Some(request::Value::InitChain(_)) => {
                response::Value::InitChain(ResponseInitChain::default())
            }
            Some(request::Value::Flush(_)) => response::Value::Flush(ResponseFlush {}),
            Some(request::Value::CheckTx(req)) => {
                let code = if req.tx.as_ref() == b"reject" { 9 } else { 0 };
                response::Value::CheckTx(ResponseCheckTx {
                    code,
                    log: if code == 0 {
                        String::new()
                    } else {
                        "rejected".to_owned()
                    },
                    ..ResponseCheckTx::default()
                })
            }
            Some(request::Value::BeginBlock(_)) => {
                response::Value::BeginBlock(ResponseBeginBlock::default())
            }
            Some(request::Value::DeliverTx(_)) => response::Value::DeliverTx(ResponseDeliverTx {
                code: 7,
                ..ResponseDeliverTx::default()
            }),
            Some(request::Value::EndBlock(_)) => {
                response::Value::EndBlock(ResponseEndBlock::default())
            }
            Some(request::Value::Commit(_)) => response::Value::Commit(ResponseCommit::default()),
            Some(request::Value::Query(_)) => response::Value::Query(ResponseQuery {
                value: b"queried".as_slice().into(),
                ..ResponseQuery::default()
            }),
            _ => response::Value::Exception(ResponseException {
                error: "unsupported".to_owned(),
            }),
        };
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
