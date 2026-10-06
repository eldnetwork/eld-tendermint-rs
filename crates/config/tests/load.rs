#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
// `std::env::set_var` is unsafe on this toolchain. This file is the only test that writes `TMHOME`.
#![allow(unsafe_code)]
//! TOML overlay, path helpers, and genesis loading.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_config::{Config, Duration, P2pConfig, load_home, load_toml, resolve_home};
use eld_tendermint_types::GenesisDoc;

const GOOD_GENESIS: &str = r#"{
    "genesis_time": "2019-10-13T16:14:44Z",
    "chain_id": "test-chain-QDKdJr",
    "initial_height": "1000",
    "consensus_params": null,
    "validators": [{
        "pub_key":{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},
        "power":"10",
        "name":""
    }],
    "app_hash":"",
    "app_state":{"account_owner": "Bob"}
}"#;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "eld-tm-config-{}-{nanos}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(path.join("config")).unwrap();
        Self(path)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn empty_toml_matches_go_defaults() {
    let loaded = load_toml(b"").unwrap();
    assert_eq!(loaded, Config::default_config());
    assert!(loaded.base.fast_sync);
    assert_eq!(loaded.base.db_backend, "goleveldb");
    assert_eq!(loaded.base.proxy_app, "tcp://127.0.0.1:26658");
    assert_eq!(loaded.consensus.timeout_propose, Duration::from_secs(3));
    assert_eq!(loaded.rpc.cors_allowed_methods, ["HEAD", "GET", "POST"]);
    assert_eq!(
        loaded.rpc.timeout_broadcast_tx_commit,
        Duration::from_secs(10)
    );
    assert_eq!(loaded.p2p.test_fuzz_config.prob_drop_rw, 0.2);
    assert_eq!(loaded.mempool.version, "v0");
    assert_eq!(loaded.fastsync.version, "v0");
    assert_eq!(loaded.tx_index.indexer, "kv");
    assert_eq!(loaded.instrumentation.namespace, "tendermint");
}

#[test]
fn partial_toml_keeps_defaults() {
    let loaded = load_toml(
        br#"
moniker = "eld-node"
proxy_app = "tcp://10.0.0.8:26658"
"#,
    )
    .unwrap();
    assert_eq!(loaded.base.moniker, "eld-node");
    assert_eq!(loaded.base.proxy_app, "tcp://10.0.0.8:26658");
    assert!(loaded.base.fast_sync);
    assert_eq!(loaded.base.db_backend, "goleveldb");
    assert_eq!(loaded.consensus.timeout_propose, Duration::from_secs(3));
    assert_eq!(loaded.base.moniker, "eld-node");
}

#[test]
fn fixture_parses_durations_hyphens_and_cors() {
    let loaded = load_toml(
        br#"
[rpc]
cors_allowed_origins = ["https://example.com", "*"]
timeout_broadcast_tx_commit = "12s"
unsafe = true

[p2p]
flush_throttle_timeout = "100ms"
handshake_timeout = "20s"

[mempool]
keep-invalid-txs-in-cache = true
ttl-duration = "1h"
ttl-num-blocks = 10

[statesync]
trust_period = "168h"

[consensus]
timeout_propose = "4s"
timeout_propose_delta = "500ms"

[tx_index]
psql-conn = "postgresql://localhost/tm"
"#,
    )
    .unwrap();
    assert_eq!(
        loaded.rpc.cors_allowed_origins,
        ["https://example.com", "*"]
    );
    assert_eq!(
        loaded.rpc.timeout_broadcast_tx_commit,
        Duration::from_secs(12)
    );
    assert!(loaded.rpc.unsafe_commands);
    assert_eq!(
        loaded.p2p.flush_throttle_timeout,
        Duration::from_millis(100)
    );
    assert_eq!(loaded.p2p.handshake_timeout, Duration::from_secs(20));
    assert!(loaded.mempool.keep_invalid_txs_in_cache);
    assert_eq!(loaded.mempool.ttl_duration, Duration::from_secs(3600));
    assert_eq!(loaded.mempool.ttl_num_blocks, 10);
    assert_eq!(
        loaded.statesync.trust_period,
        Duration::from_secs(168 * 60 * 60)
    );
    assert_eq!(loaded.consensus.timeout_propose, Duration::from_secs(4));
    assert_eq!(
        loaded.consensus.timeout_propose_delta,
        Duration::from_millis(500)
    );
    assert_eq!(loaded.tx_index.psql_conn, "postgresql://localhost/tm");
    assert_eq!(loaded.rpc.cors_allowed_methods, ["HEAD", "GET", "POST"]);
}

#[test]
fn unknown_keys_are_ignored() {
    let loaded = load_toml(
        br#"
moniker = "x"
not_a_real_key = 1
[rpc]
also_unknown = true
"#,
    )
    .unwrap();
    assert_eq!(loaded.base.moniker, "x");
    assert_eq!(loaded.rpc.laddr, "tcp://127.0.0.1:26657");
}

#[test]
fn bad_duration_string_fails() {
    let err = load_toml(b"[consensus]\ntimeout_propose = \"nope\"\n").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("nope"), "{message}");
    assert!(load_toml(b"[consensus]\ntimeout_propose = 3\n").is_err());
}

#[test]
fn negative_duration_parses() {
    let loaded = load_toml(b"[consensus]\ntimeout_propose = \"-1s\"\n").unwrap();
    assert_eq!(loaded.consensus.timeout_propose, Duration::from_secs(-1));
    assert!(loaded.validate_basic().is_err());
}

#[test]
fn paths_match_go_default_config_test() {
    let mut cfg = Config::default_config();
    cfg.set_root("/foo");
    cfg.base.genesis_file = "bar".to_owned();
    cfg.base.db_dir = "/opt/data".to_owned();
    cfg.mempool.wal_dir = "wal/mem/".to_owned();
    assert_eq!(cfg.genesis_file(), PathBuf::from("/foo/bar"));
    assert_eq!(cfg.db_dir(), PathBuf::from("/opt/data"));
    assert_eq!(cfg.mempool.wal_dir(), PathBuf::from("/foo/wal/mem"));
    assert_eq!(
        cfg.consensus.wal_file(),
        PathBuf::from("/foo/data/cs.wal/wal")
    );
    cfg.consensus.set_wal_file("/abs/wal");
    assert_eq!(cfg.consensus.wal_file(), PathBuf::from("/abs/wal"));
}

#[test]
fn tls_paths_match_go() {
    let mut cfg = Config::default_config();
    cfg.set_root("/home/user");
    cfg.rpc.tls_cert_file = "file.crt".to_owned();
    assert_eq!(
        cfg.rpc.cert_file(),
        PathBuf::from("/home/user/config/file.crt")
    );
    cfg.rpc.tls_key_file = "file.key".to_owned();
    assert_eq!(
        cfg.rpc.key_file(),
        PathBuf::from("/home/user/config/file.key")
    );
    cfg.rpc.tls_cert_file = "/abs/path/to/file.crt".to_owned();
    assert_eq!(cfg.rpc.cert_file(), PathBuf::from("/abs/path/to/file.crt"));
    cfg.rpc.tls_key_file = "/abs/path/to/file.key".to_owned();
    assert_eq!(cfg.rpc.key_file(), PathBuf::from("/abs/path/to/file.key"));
    assert!(cfg.rpc.is_tls_enabled());
}

#[test]
fn empty_section_keeps_section_defaults() {
    let loaded = load_toml(b"[rpc]\n").unwrap();
    assert_eq!(loaded.rpc, Config::default_config().rpc);
    assert_eq!(loaded.consensus, Config::default_config().consensus);
}

#[test]
fn home_dir_loads_genesis_validator_set() {
    let home = TempHome::new("genesis");
    fs::write(
        home.0.join("config").join("config.toml"),
        "moniker = \"eld-node\"\nproxy_app = \"tcp://10.0.0.8:26658\"\n",
    )
    .unwrap();
    fs::write(home.0.join("config").join("genesis.json"), GOOD_GENESIS).unwrap();
    let config = load_home(&home.0).unwrap();
    assert_eq!(config.base.moniker, "eld-node");
    assert_eq!(config.base.root_dir, home.0.to_string_lossy());
    let doc = config.load_genesis().unwrap();
    assert_eq!(doc.chain_id.as_str(), "test-chain-QDKdJr");
    assert_eq!(doc.validators.len(), 1);
    assert_eq!(doc.validators[0].power, 10);
    let direct = GenesisDoc::from_json(GOOD_GENESIS.as_bytes()).unwrap();
    assert_eq!(doc.validators[0].address, direct.validators[0].address);
}

#[test]
fn home_root_config_toml_wins_over_nested_file() {
    let home = TempHome::new("order");
    fs::write(home.0.join("config.toml"), "moniker = \"root\"\n").unwrap();
    fs::write(
        home.0.join("config").join("config.toml"),
        "moniker = \"nested\"\n",
    )
    .unwrap();
    fs::write(home.0.join("config").join("genesis.json"), GOOD_GENESIS).unwrap();
    let config = load_home(&home.0).unwrap();
    assert_eq!(config.base.moniker, "root");
}

#[test]
fn explicit_false_overrides_default() {
    let loaded = load_toml(b"fast_sync = false\n").unwrap();
    assert!(!loaded.base.fast_sync);
}

#[test]
fn home_flag_wins_over_tmhome() {
    let _guard = ENV_LOCK.lock().unwrap();
    // SAFETY: this test is the only one that writes the process environment, and it holds ENV_LOCK.
    unsafe {
        std::env::set_var("TMHOME", "/from-env");
    }
    assert_eq!(
        resolve_home(Some("/from-flag")).unwrap(),
        PathBuf::from("/from-flag")
    );
    assert_eq!(resolve_home(None).unwrap(), PathBuf::from("/from-env"));
    unsafe {
        std::env::remove_var("TMHOME");
    }
}

#[test]
fn rpc_servers_empty_string_loads_as_empty_list() {
    let loaded = load_toml(b"[statesync]\nrpc_servers = \"\"\n").unwrap();
    assert!(loaded.statesync.rpc_servers.is_empty());
}

#[test]
fn rpc_servers_comma_separated_string_loads_as_two_servers() {
    let loaded = load_toml(
        br#"
[statesync]
rpc_servers = "http://a:26657,http://b:26657"
"#,
    )
    .unwrap();
    assert_eq!(
        loaded.statesync.rpc_servers,
        ["http://a:26657", "http://b:26657"]
    );
}

#[test]
fn rpc_servers_toml_array_stays_a_list() {
    let loaded = load_toml(
        br#"
[statesync]
rpc_servers = ["http://a:26657", "http://b:26657"]
"#,
    )
    .unwrap();
    assert_eq!(
        loaded.statesync.rpc_servers,
        ["http://a:26657", "http://b:26657"]
    );
}

#[test]
fn rpc_servers_rejects_non_string_and_names_the_field() {
    let err = load_toml(b"[statesync]\nrpc_servers = 1\n").unwrap_err();
    let message = err.to_string();
    assert!(message.contains("rpc_servers"), "{message}");
}

#[test]
fn p2p_addr_book_is_rooted() {
    let mut cfg = P2pConfig::default_config();
    cfg.root_dir = "/foo".to_owned();
    assert_eq!(
        cfg.addr_book_file(),
        PathBuf::from("/foo/config/addrbook.json")
    );
}
