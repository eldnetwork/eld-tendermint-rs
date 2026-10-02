//! `eld-tendermint start` serves genesis `status` at height 0.

use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eld_tendermint_abci::{read_message, write_message};
use eld_tendermint_crypto::marshal_pub_key;
use eld_tendermint_p2p::NodeKey;
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::{
    Request, Response, ResponseBeginBlock, ResponseCheckTx, ResponseCommit, ResponseDeliverTx,
    ResponseEndBlock, ResponseException, ResponseFlush, ResponseInfo, request, response,
};
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

    let (_code, _headers, unknown) = post(
        &addr,
        r#"{"jsonrpc":"2.0","id":1,"method":"broadcast_tx_commit"}"#,
    );
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
    let pv = FilePV::load_or_gen_file_pv(
        home.join("config/priv_validator_key.json"),
        home.join("data/priv_validator_state.json"),
    )
    .unwrap();
    NodeKey::load_or_gen(home.join("config/node_key.json")).unwrap();
    let pub_key: Value = serde_json::from_str(&marshal_pub_key(&pv.get_pub_key())).unwrap();
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
        "proxy_app = {proxy_app:?}\nmoniker = \"status-node\"\n\n[rpc]\nladdr = {rpc_laddr:?}\n\n[p2p]\nladdr = \"tcp://127.0.0.1:0\"\npersistent_peers = \"\"\n"
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
            Some(request::Value::Flush(_)) => response::Value::Flush(ResponseFlush {}),
            Some(request::Value::CheckTx(_)) => {
                response::Value::CheckTx(ResponseCheckTx::default())
            }
            Some(request::Value::BeginBlock(_)) => {
                response::Value::BeginBlock(ResponseBeginBlock::default())
            }
            Some(request::Value::DeliverTx(_)) => {
                response::Value::DeliverTx(ResponseDeliverTx::default())
            }
            Some(request::Value::EndBlock(_)) => {
                response::Value::EndBlock(ResponseEndBlock::default())
            }
            Some(request::Value::Commit(_)) => response::Value::Commit(ResponseCommit::default()),
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
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
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
