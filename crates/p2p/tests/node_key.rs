#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `node_key.json` load and save.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_crypto::{PrivKey, marshal_priv_key};
use eld_tendermint_p2p::NodeKey;

fn temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!(
        "eld-node-key-{name}-{}-{nanos}.json",
        std::process::id()
    ));
    path
}

#[test]
fn load_amino_fixture_and_round_trip() {
    let priv_key = PrivKey::generate();
    let pub_key = priv_key.public_key().expect("public key");
    let fixture = temp_path("fixture");
    let json = format!(r#"{{"priv_key":{}}}"#, marshal_priv_key(&priv_key));
    fs::write(&fixture, json).expect("write fixture");

    let loaded = NodeKey::load(&fixture).expect("load");
    assert_eq!(loaded.priv_key, priv_key);
    assert_eq!(loaded.id().expect("id"), hex::encode(pub_key.address()));

    let saved = temp_path("saved");
    loaded.save_as(&saved).expect("save");
    let mode = fs::metadata(&saved).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    let again = NodeKey::load(&saved).expect("reload");
    assert_eq!(again.priv_key, priv_key);

    let generated = temp_path("generated");
    let first = NodeKey::load_or_gen(&generated).expect("generate");
    let second = NodeKey::load_or_gen(&generated).expect("reload generated");
    assert_eq!(first.priv_key, second.priv_key);

    let _ = fs::remove_file(fixture);
    let _ = fs::remove_file(saved);
    let _ = fs::remove_file(generated);
}

#[test]
fn load_missing_file_fails() {
    let path = temp_path("missing");
    let _ = fs::remove_file(&path);
    assert!(NodeKey::load(&path).is_err());
}
