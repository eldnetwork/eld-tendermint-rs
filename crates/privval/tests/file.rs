//! FilePV cases from `privval/file_test.go`, with a synthesized Amino key file.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_crypto::{PrivKey, marshal_priv_key, marshal_pub_key};
use eld_tendermint_privval::{Error, FilePV};
use eld_tendermint_types::{BlockId, PartSetHeader, SignedMsgType, Time, Vote};

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("eld-pv-{}-{name}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn block(byte: u8) -> BlockId {
    BlockId {
        hash: vec![byte; 32],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![byte.wrapping_add(1); 32],
        },
    }
}

fn vote(height: i64, block_id: BlockId) -> Vote {
    Vote {
        vote_type: SignedMsgType::Prevote,
        height,
        round: 0,
        block_id,
        timestamp: Time::from_unix_parts(1_700_000_000, 0),
        validator_address: Vec::new(),
        validator_index: 0,
        signature: Vec::new(),
    }
}

#[test]
fn synthesized_key_file_loads() {
    let dir = temp_dir("load");
    let key_path = dir.join("priv_validator_key.json");
    let state_path = dir.join("priv_validator_state.json");
    let priv_key = PrivKey::generate();
    let pub_key = priv_key.public_key().unwrap();
    let key_json = format!(
        "{{\n  \"address\": \"{}\",\n  \"pub_key\": {},\n  \"priv_key\": {}\n}}\n",
        hex::encode_upper(pub_key.address()),
        marshal_pub_key(&pub_key),
        marshal_priv_key(&priv_key),
    );
    fs::write(&key_path, key_json).unwrap();
    fs::write(&state_path, r#"{"height":"0","round":0,"step":0}"#).unwrap();

    let loaded = FilePV::load(&key_path, &state_path).unwrap();
    assert_eq!(loaded.get_pub_key(), pub_key);
    assert_eq!(loaded.key.address, pub_key.address());
    assert_eq!(loaded.key.priv_key, priv_key);
    assert_eq!(loaded.last_sign_state.height, 0);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn replay_is_stable_and_conflict_is_rejected() {
    let dir = temp_dir("replay");
    let key_path = dir.join("priv_validator_key.json");
    let state_path = dir.join("priv_validator_state.json");
    let mut pv = FilePV::generate(&key_path, &state_path);
    pv.save().unwrap();

    let mut vote = vote(1, block(1));
    pv.sign_vote("chain", &mut vote).unwrap();
    vote.verify_signature(&pv.get_pub_key(), "chain").unwrap();
    let first_sig = vote.signature.clone();
    assert!(
        fs::read_to_string(&state_path)
            .unwrap()
            .contains("\"height\": \"1\"")
    );

    let mut reloaded = FilePV::load(&key_path, &state_path).unwrap();
    let mut again = vote.clone();
    again.signature.clear();
    reloaded.sign_vote("chain", &mut again).unwrap();
    assert_eq!(again.signature, first_sig);

    let mut conflict = vote.clone();
    conflict.signature.clear();
    conflict.block_id = block(9);
    assert!(matches!(
        reloaded.sign_vote("chain", &mut conflict),
        Err(Error::ConflictingData)
    ));

    let mut next = vote.clone();
    next.height = 2;
    next.signature.clear();
    reloaded.sign_vote("chain", &mut next).unwrap();
    next.verify_signature(&reloaded.get_pub_key(), "chain")
        .unwrap();
    assert_ne!(next.signature, first_sig);
    let state = fs::read_to_string(&state_path).unwrap();
    assert!(state.contains("\"height\": \"2\""));
    assert_eq!(reloaded.last_sign_state.height, 2);
    let _ = fs::remove_dir_all(dir);
}
