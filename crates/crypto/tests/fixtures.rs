#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Golden vectors from the Go tree.
//!
//! Amino strings are the Ed25519 rows in `crypto/README.md`.
//! The `"abc"` hashes are `crypto/tmhash/hash_test.go`.

use eld_tendermint_proto::crypto::PublicKey as ProtoPublicKey;
use eld_tendermint_proto::crypto::public_key::Sum;

use eld_tendermint_crypto::{
    Error, PRIV_KEY_NAME, PUB_KEY_NAME, PrivKey, PubKey, address_hash, marshal_priv_key,
    marshal_pub_key, pub_key_from_proto, pub_key_to_proto, sum, sum_truncated, unmarshal_priv_key,
    unmarshal_pub_key,
};

const PUB_B64: &str = "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=";
const PRIV_B64: &str =
    "EVkqJO/jIXp3rkASXfh9YnyToYXRXhBr6g9cQVxPFnQBP/5povV4HTjvsy530kybxKHwEi85iU8YL0qQhSYVoQ==";

const PUB_JSON: &str = concat!(
    r#"{"type":"tendermint/PubKeyEd25519","value":""#,
    "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=",
    r#""}"#,
);
const PRIV_JSON: &str = concat!(
    r#"{"type":"tendermint/PrivKeyEd25519","value":""#,
    "EVkqJO/jIXp3rkASXfh9YnyToYXRXhBr6g9cQVxPFnQBP/5povV4HTjvsy530kybxKHwEi85iU8YL0qQhSYVoQ==",
    r#""}"#,
);

const PUB_HEX: &str = "013ffe69a2f5781d38efb32e77d24c9bc4a1f0122f39894f182f4a90852615a1";
const ADDRESS_HEX: &str = "a3258dcbf45dca0df052981870f2d1441a36d145";

#[test]
fn tmhash_abc_matches_go() {
    let abc = b"abc";
    assert_eq!(
        hex::encode(sum(abc)),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex::encode(sum_truncated(abc)),
        "ba7816bf8f01cfea414140de5dae2223b00361a3"
    );
}

#[test]
fn readme_keys_round_trip_amino_and_address() {
    let pub_key = unmarshal_pub_key(PUB_JSON).unwrap();
    let priv_key = unmarshal_priv_key(PRIV_JSON).unwrap();

    assert_eq!(hex::encode(pub_key.as_bytes()), PUB_HEX);
    assert_eq!(priv_key.as_bytes().len(), 64);
    assert_eq!(&priv_key.as_bytes()[32..], pub_key.as_bytes().as_slice());
    assert_eq!(priv_key.public_key().unwrap(), pub_key);

    assert_eq!(hex::encode(pub_key.address()), ADDRESS_HEX);
    assert_eq!(address_hash(pub_key.as_bytes()), pub_key.address());

    assert_eq!(marshal_pub_key(&pub_key), PUB_JSON);
    assert_eq!(marshal_priv_key(&priv_key), PRIV_JSON);
    assert!(PUB_JSON.contains(PUB_KEY_NAME));
    assert!(PRIV_JSON.contains(PRIV_KEY_NAME));
    assert!(PUB_JSON.contains(PUB_B64));
    assert!(PRIV_JSON.contains(PRIV_B64));
}

#[test]
fn amino_accepts_whitespace_and_either_field_order() {
    let spaced = format!("{{\n  \"type\": \"{PUB_KEY_NAME}\",\n  \"value\": \"{PUB_B64}\"\n}}");
    let swapped = format!(r#"{{"value":"{PUB_B64}","type":"{PUB_KEY_NAME}"}}"#);
    let from_spaced = unmarshal_pub_key(&spaced).unwrap();
    let from_swapped = unmarshal_pub_key(&swapped).unwrap();
    assert_eq!(from_spaced, from_swapped);
    assert_eq!(hex::encode(from_spaced.as_bytes()), PUB_HEX);
}

#[test]
fn amino_rejects_bad_envelopes() {
    let wrong_type = format!(r#"{{"type":"tendermint/PubKeySecp256k1","value":"{PUB_B64}"}}"#);
    assert_eq!(
        unmarshal_pub_key(&wrong_type),
        Err(Error::WrongAminoType {
            expected: PUB_KEY_NAME,
            got: "tendermint/PubKeySecp256k1".to_string(),
        })
    );

    let bad_b64 = format!(r#"{{"type":"{PUB_KEY_NAME}","value":"!!!!"}}"#);
    assert_eq!(unmarshal_pub_key(&bad_b64), Err(Error::InvalidBase64));

    let missing_padding = format!(
        r#"{{"type":"{PUB_KEY_NAME}","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE"}}"#
    );
    assert_eq!(
        unmarshal_pub_key(&missing_padding),
        Err(Error::InvalidBase64)
    );

    assert_eq!(
        unmarshal_pub_key(&format!(
            r#"{{"type":"{PUB_KEY_NAME}","value":"{PRIV_B64}"}}"#
        )),
        Err(Error::InvalidLength {
            expected: 32,
            got: 64,
        })
    );
    assert_eq!(
        unmarshal_priv_key(&format!(
            r#"{{"type":"{PRIV_KEY_NAME}","value":"{PUB_B64}"}}"#
        )),
        Err(Error::InvalidLength {
            expected: 64,
            got: 32,
        })
    );

    assert_eq!(unmarshal_pub_key("{}"), Err(Error::InvalidAminoJson));
    assert_eq!(
        unmarshal_pub_key(&format!(
            r#"{{"type":"{PUB_KEY_NAME}","value":"{PUB_B64}",}}"#
        )),
        Err(Error::InvalidAminoJson)
    );
}

#[test]
fn key_bytes_reject_wrong_lengths_and_zero_suffix() {
    assert_eq!(
        PubKey::from_bytes(&[0; 31]),
        Err(Error::InvalidLength {
            expected: 32,
            got: 31,
        })
    );
    assert_eq!(
        PrivKey::from_bytes(&[0; 32]),
        Err(Error::InvalidLength {
            expected: 64,
            got: 32,
        })
    );

    let mut raw = [0u8; 64];
    raw[..32].copy_from_slice(&[0x11; 32]);
    let key = PrivKey::from_bytes(&raw).unwrap();
    assert_eq!(key.public_key(), Err(Error::UninitializedPrivKey));
}

#[test]
fn proto_public_key_round_trip() {
    let pub_key = unmarshal_pub_key(PUB_JSON).unwrap();
    let proto = pub_key_to_proto(&pub_key);
    assert_eq!(proto.sum, Some(Sum::Ed25519(pub_key.as_bytes().to_vec())));
    assert_eq!(pub_key_from_proto(&proto).unwrap(), pub_key);

    assert_eq!(
        pub_key_from_proto(&ProtoPublicKey {
            sum: Some(Sum::Ed25519(vec![1, 2, 3])),
        }),
        Err(Error::InvalidLength {
            expected: 32,
            got: 3,
        })
    );
    assert_eq!(
        pub_key_from_proto(&ProtoPublicKey {
            sum: Some(Sum::Secp256k1(vec![0; 33])),
        }),
        Err(Error::UnsupportedKeyType)
    );
    assert_eq!(
        pub_key_from_proto(&ProtoPublicKey { sum: None }),
        Err(Error::UnsupportedKeyType)
    );
}
