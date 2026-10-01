//! `crypto/encoding`: Ed25519 keys as `tendermint.crypto.PublicKey`.
//!
//! The secp256k1 oneof arm is rejected until that curve is ported.

use eld_tendermint_proto::crypto::PublicKey as ProtoPublicKey;
use eld_tendermint_proto::crypto::public_key::Sum;

use crate::Error;
use crate::ed25519::PubKey;

/// `encoding.PubKeyToProto` for an Ed25519 key.
#[must_use]
pub fn pub_key_to_proto(key: &PubKey) -> ProtoPublicKey {
    ProtoPublicKey {
        sum: Some(Sum::Ed25519(key.as_bytes().to_vec())),
    }
}

/// `encoding.PubKeyFromProto` for an Ed25519 key.
///
/// # Errors
///
/// Returns [`Error::InvalidLength`] when the Ed25519 bytes are not 32 long.
/// Returns [`Error::UnsupportedKeyType`] when the sum is absent or is secp256k1.
pub fn pub_key_from_proto(key: &ProtoPublicKey) -> Result<PubKey, Error> {
    match &key.sum {
        Some(Sum::Ed25519(bytes)) => PubKey::from_bytes(bytes),
        Some(Sum::Secp256k1(_)) | None => Err(Error::UnsupportedKeyType),
    }
}
