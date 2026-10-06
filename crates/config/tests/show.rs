#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! The `eld-tendermint-config` binary prints chain id, moniker, proxy app, and validators.

use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

#[test]
fn binary_prints_mounted_home() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let home = std::env::temp_dir().join(format!("eld-tm-show-{}-{nanos}", std::process::id()));
    fs::create_dir_all(home.join("config")).unwrap();
    fs::write(
        home.join("config").join("config.toml"),
        "moniker = \"eld-node\"\nproxy_app = \"tcp://10.0.0.8:26658\"\n",
    )
    .unwrap();
    fs::write(home.join("config").join("genesis.json"), GOOD_GENESIS).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_eld-tendermint-config"))
        .arg("--home")
        .arg(&home)
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&home);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(output.status.success(), "stderr={stderr} stdout={stdout}");

    let doc = GenesisDoc::from_json(GOOD_GENESIS.as_bytes()).unwrap();
    let address = hex::encode_upper(&doc.validators[0].address);
    let expected = format!(
        "chain_id: test-chain-QDKdJr\nmoniker: eld-node\nproxy_app: tcp://10.0.0.8:26658\nvalidators:\n  {address} power=10 name=\n"
    );
    assert_eq!(stdout, expected);
}
