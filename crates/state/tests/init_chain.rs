//! `InitChain` against an in-process response. No socket.

use std::cell::Cell;

use eld_tendermint_crypto::{PrivKey, PubKey, hash_from_byte_slices, pub_key_to_proto};
use eld_tendermint_proto::abci::{ResponseInitChain, ValidatorUpdate};
use eld_tendermint_store::MemDb;
use eld_tendermint_types::{ChainId, GenesisDoc, GenesisValidator, Time, ValidatorSet};
use prost::Message;
use prost::bytes::Bytes;

use eld_tendermint_state::{StateStore, load_or_init_chain};

const APP_HASH: [u8; 32] = [0xab; 32];

fn key() -> PubKey {
    PrivKey::generate()
        .public_key()
        .expect("ed25519 public key")
}

fn genesis(pub_key: PubKey) -> GenesisDoc {
    GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: None,
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key,
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    }
}

fn contains(set: &ValidatorSet, address: &[u8]) -> bool {
    set.validators()
        .iter()
        .any(|validator| validator.address == address)
}

fn init_response() -> ResponseInitChain {
    ResponseInitChain {
        app_hash: Bytes::copy_from_slice(&APP_HASH),
        ..ResponseInitChain::default()
    }
}

#[test]
fn empty_store_calls_init_chain_once_and_saves_app_hash() {
    let store = StateStore::new(MemDb::new());
    let mut doc = genesis(key());
    let calls = Cell::new(0);
    let state = load_or_init_chain(&store, &mut doc, |_| {
        calls.set(calls.get() + 1);
        Ok(init_response())
    })
    .expect("init");
    assert_eq!(calls.get(), 1);
    assert_eq!(state.app_hash, APP_HASH);
    let loaded = store.load().expect("stateKey");
    assert_eq!(loaded.app_hash, APP_HASH);
    assert_eq!(
        loaded.last_results_hash,
        hash_from_byte_slices::<&[u8]>(&[])
    );
}

#[test]
fn second_open_does_not_call_init_chain() {
    let store = StateStore::new(MemDb::new());
    let mut doc = genesis(key());
    let calls = Cell::new(0);
    load_or_init_chain(&store, &mut doc, |_| {
        calls.set(calls.get() + 1);
        Ok(init_response())
    })
    .expect("first open");
    let loaded = load_or_init_chain(&store, &mut doc, |_| {
        calls.set(calls.get() + 1);
        Ok(init_response())
    })
    .expect("second open");
    assert_eq!(calls.get(), 1);
    assert_eq!(loaded.app_hash, APP_HASH);
}

#[test]
fn validator_update_is_next_not_current() {
    let genesis_key = key();
    let added = key();
    let store = StateStore::new(MemDb::new());
    let mut doc = genesis(genesis_key);
    let state = load_or_init_chain(&store, &mut doc, |_| {
        Ok(ResponseInitChain {
            app_hash: Bytes::copy_from_slice(&APP_HASH),
            validators: vec![ValidatorUpdate {
                pub_key: Some(pub_key_to_proto(&added)),
                power: 20,
            }],
            consensus_params: None,
        })
    })
    .expect("init");
    let added_address = added.address();
    let genesis_address = doc.validators[0].address.clone();
    assert_eq!(state.next_validators.validators().len(), 1);
    assert!(contains(&state.next_validators, added_address.as_slice()));
    assert!(!contains(&state.validators, added_address.as_slice()));
    assert!(contains(&state.validators, &genesis_address));
    assert_eq!(state.validators.validators().len(), 1);
}

#[test]
fn empty_app_state_bytes_are_omitted() {
    let store = StateStore::new(MemDb::new());
    let mut doc = genesis(key());
    let seen = Cell::new(None);
    load_or_init_chain(&store, &mut doc, |request| {
        seen.set(Some(request));
        Ok(init_response())
    })
    .expect("init");
    let request = seen.take().expect("request");
    assert!(request.app_state_bytes.is_empty());
    assert_eq!(request.initial_height, 1);
    assert!(
        request
            .consensus_params
            .as_ref()
            .and_then(|params| params.version.as_ref())
            .is_none()
    );
    let fields = top_level_fields(&request.encode_to_vec());
    assert!(
        !fields.contains(&5),
        "empty app_state_bytes was written as field 5: {fields:?}"
    );

    let store = StateStore::new(MemDb::new());
    let mut doc = genesis(key());
    doc.app_state = Some(serde_json::json!({"account_owner": "Bob"}));
    let seen = Cell::new(None);
    load_or_init_chain(&store, &mut doc, |request| {
        seen.set(Some(request));
        Ok(init_response())
    })
    .expect("init with app state");
    let request = seen.take().expect("request");
    let json = br#"{"account_owner":"Bob"}"#;
    assert_eq!(request.app_state_bytes.as_ref(), json);
    let encoded = request.encode_to_vec();
    assert!(
        encoded.windows(json.len()).any(|window| window == json),
        "app_state_bytes missing from {}",
        hex_bytes(&encoded)
    );
    assert!(top_level_fields(&encoded).contains(&5));
}

fn top_level_fields(bytes: &[u8]) -> Vec<u64> {
    let mut fields = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (key, next) = read_varint(bytes, i);
        i = next;
        fields.push(key >> 3);
        i = match key & 7 {
            0 => read_varint(bytes, i).1,
            1 => i + 8,
            2 => {
                let (len, next) = read_varint(bytes, i);
                next + usize::try_from(len).expect("field length")
            }
            5 => i + 4,
            wire => panic!("wire type {wire}"),
        };
    }
    fields
}

fn read_varint(bytes: &[u8], mut i: usize) -> (u64, usize) {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[i];
        i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return (value, i);
        }
        shift += 7;
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
