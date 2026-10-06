//! `p2p.NodeKey`.
//!
//! `node_key.json` stores only `priv_key`, as Amino JSON. The peer id is not in the
//! file. [`NodeKey::id`] is the lowercase hex of the public-key address (`encoding/hex`).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use eld_tendermint_crypto::{PrivKey, PubKey, marshal_priv_key, unmarshal_priv_key};

use crate::Error;
use crate::ensured::Ensured;

/// `p2p.NodeKey`. The persistent peer authentication key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeKey {
    pub priv_key: PrivKey,
}

impl NodeKey {
    /// `NodeKey.PubKey`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Crypto`] when the private-key suffix is uninitialized.
    pub fn pub_key(&self) -> Result<PubKey, Error> {
        self.priv_key.public_key().map_err(Error::Crypto)
    }

    /// `NodeKey.ID`: lowercase hex of the public-key address.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::pub_key`].
    pub fn id(&self) -> Result<String, Error> {
        Ok(hex::encode(self.pub_key()?.address()))
    }

    /// `LoadNodeKey`.
    ///
    /// # Errors
    ///
    /// Returns an error when the file is missing, the JSON has no `priv_key`, or the
    /// Amino envelope is not an Ed25519 private key.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let bytes = fs::read(path).map_err(Error::Io)?;
        let file: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|err| Error::Json(err.to_string()))?;
        let Some(priv_key) = file.get("priv_key") else {
            return Err(Error::Json("missing priv_key".to_owned()));
        };
        let priv_json =
            serde_json::to_string(priv_key).map_err(|err| Error::Json(err.to_string()))?;
        let priv_key = unmarshal_priv_key(&priv_json).map_err(Error::Crypto)?;
        Ok(Self { priv_key })
    }

    /// `NodeKey.SaveAs`. Writes compact JSON and sets mode `0600` on Unix.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be created or written.
    ///
    /// # Panics
    ///
    /// Panics when the Amino private key is not a JSON object. Marshaling an Ed25519
    /// private key always produces that object.
    pub fn save_as(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let priv_key: serde_json::Value = serde_json::from_str(&marshal_priv_key(&self.priv_key))
            .ensured("amino private key json is an object");
        let bytes = serde_json::to_vec(&serde_json::json!({ "priv_key": priv_key }))
            .ensured("node key json");
        write_secret(path.as_ref(), &bytes)
    }

    /// `LoadOrGenNodeKey`. Loads the file when it exists. Otherwise generates a key and saves it.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::load`] or [`Self::save_as`].
    pub fn load_or_gen(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        if path.exists() {
            return Self::load(path);
        }
        let node_key = Self {
            priv_key: PrivKey::generate(),
        };
        node_key.save_as(path)?;
        Ok(node_key)
    }
}

fn write_secret(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(Error::Io)?;
    file.write_all(bytes).map_err(Error::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(Error::Io)?;
    }
    Ok(())
}
