//! Eld capacity-validator account key.
//!
//! This is not the peer key and not the consensus key. It is the same Ed25519
//! seed the ABCI app keeps for `wallet-capacity-validator-*`. Signing matches
//! `eld_common::tx::Tx::sign`: the message is the JSON of the transaction with
//! an empty `sig`, followed by the chain id as UTF-8.

use std::fs;
use std::io;
use std::path::Path;

use eld_tendermint_crypto::{PrivKey, address_hash};
use serde::Deserialize;

/// One capacity-validator account key loaded from `capacity_validator_key.json`.
///
/// `sign_eld_tx` is the call that builds a summary transaction. Until that path
/// lands, the binary only loads the key and logs its address.
#[derive(Clone)]
#[allow(dead_code)]
pub struct CapacityValidatorKey {
    key: PrivKey,
    public_key: [u8; 32],
}

#[derive(Deserialize)]
struct FileKey {
    public_key: String,
    private_key: String,
    keytype: String,
}

#[derive(Deserialize)]
struct FileWallet {
    keypair: FileKey,
}

#[allow(dead_code)]
impl CapacityValidatorKey {
    /// Loads the Eld wallet keypair. The public key must match the seed.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is missing, the JSON is not a keypair,
    /// the hex is not 32 bytes, or the public key does not match the seed.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                format!("capacity validator key not found at {}", path.display())
            } else {
                err.to_string()
            }
        })?;
        let wallet: FileWallet = serde_json::from_str(&text)
            .map_err(|err| format!("capacity validator key is not valid JSON: {err}"))?;
        Self::from_keypair(&wallet.keypair)
    }

    fn from_keypair(keypair: &FileKey) -> Result<Self, String> {
        if keypair.keytype != "ed25519" && keypair.keytype != "ed25519_keypair" {
            return Err(format!(
                "capacity validator key type {} is not ed25519",
                keypair.keytype
            ));
        }
        let seed = decode_32(&keypair.private_key, "private_key")?;
        let stated = decode_32(&keypair.public_key, "public_key")?;
        let key = PrivKey::from_seed(&seed);
        let public_key = key.public_key().map_err(|err| err.to_string())?;
        if public_key.as_bytes() != &stated {
            return Err("capacity validator public key does not match the private key".to_owned());
        }
        Ok(Self {
            key,
            public_key: stated,
        })
    }

    /// Verifying key bytes.
    #[must_use]
    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    /// Account address: first 20 bytes of SHA-256 of the verifying key, lowercase hex, no `0x`.
    #[must_use]
    pub fn address_hex(&self) -> String {
        hex::encode(address_hash(&self.public_key))
    }

    /// Signs an Eld transaction.
    ///
    /// `unsigned_tx_json` is `serde_json` of the transaction with `sig` empty.
    /// The signed message is those bytes followed by `chain_id` as UTF-8.
    /// The returned string is 128 lowercase hex characters, the form `Tx.sig` stores.
    ///
    /// # Errors
    ///
    /// Returns an error when the key cannot sign. A key loaded by [`Self::load`] can sign.
    pub fn sign_eld_tx(&self, unsigned_tx_json: &str, chain_id: &str) -> Result<String, String> {
        let mut message = unsigned_tx_json.as_bytes().to_vec();
        message.extend_from_slice(chain_id.as_bytes());
        let signature = self.key.sign(&message).map_err(|err| err.to_string())?;
        Ok(hex::encode(signature))
    }
}

fn decode_32(text: &str, field: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(text).map_err(|err| format!("{field} is not hex: {err}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("{field} must be 32 bytes"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::CapacityValidatorKey;

    #[test]
    fn signs_the_json_plus_chain_id() {
        let generated = eld_tendermint_crypto::PrivKey::generate();
        let public_key = generated.public_key().expect("public key");
        let seed = &generated.as_bytes()[..32];
        let raw = format!(
            r#"{{"keypair":{{"public_key":"{}","private_key":"{}","keytype":"ed25519"}}}}"#,
            hex::encode(public_key.as_bytes()),
            hex::encode(seed)
        );
        let dir = std::env::temp_dir().join(format!(
            "eld-capacity-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|dur| dur.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("capacity_validator_key.json");
        std::fs::write(&path, raw).expect("write");
        let key = CapacityValidatorKey::load(&path).expect("load");
        assert_eq!(key.public_key(), public_key.as_bytes());
        assert_eq!(
            key.address_hex(),
            hex::encode(eld_tendermint_crypto::address_hash(public_key.as_bytes()))
        );
        let json = r#"{"sig":"","nonce":1}"#;
        let signature = key.sign_eld_tx(json, "test-chain").expect("sign");
        assert_eq!(signature.len(), 128);
        let other = key.sign_eld_tx(json, "other-chain").expect("sign");
        assert_ne!(signature, other);
    }
}
