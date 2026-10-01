//! Ed25519 sign/verify. The Go test generates a random key, so the fixed vector
//! is RFC 8032 test 2, which `golang.org/x/crypto/ed25519` also implements.

use eld_tendermint_crypto::{Error, PrivKey, PubKey};

const SEED_HEX: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
const PUB_HEX: &str = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
const SIG_HEX: &str = "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00";

fn rfc_key() -> PrivKey {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(&hex::decode(SEED_HEX).unwrap());
    bytes[32..].copy_from_slice(&hex::decode(PUB_HEX).unwrap());
    PrivKey::from_bytes(&bytes).unwrap()
}

#[test]
fn rfc8032_signature_matches_go_layout() {
    let key = rfc_key();
    let sig = key.sign(&[0x72]).unwrap();
    assert_eq!(hex::encode(sig), SIG_HEX);
    let pub_key = key.public_key().unwrap();
    assert_eq!(hex::encode(pub_key.as_bytes()), PUB_HEX);
    pub_key.verify(&[0x72], &sig).unwrap();
}

#[test]
fn generated_key_signs_and_rejects_a_mutated_signature() {
    let key = PrivKey::generate();
    let pub_key = key.public_key().unwrap();
    assert_eq!(key.as_bytes()[32..], pub_key.as_bytes()[..]);
    let message = b"tendermint-ed25519";
    let mut signature = key.sign(message).unwrap();
    pub_key.verify(message, &signature).unwrap();
    signature[7] ^= 0x01;
    assert_eq!(
        pub_key.verify(message, &signature),
        Err(Error::BadSignature)
    );
    assert!(matches!(
        pub_key.verify(message, &signature[..63]),
        Err(Error::InvalidSignatureLength { got: 63 })
    ));
}

#[test]
fn mismatched_suffix_cannot_sign() {
    let mut bytes = rfc_key().as_bytes().to_owned();
    bytes[32] ^= 0x01;
    let key = PrivKey::from_bytes(&bytes).unwrap();
    assert_eq!(key.sign(b"msg"), Err(Error::PrivKeyMismatch));
    let _ = PubKey::from_bytes(&bytes[32..]);
}
