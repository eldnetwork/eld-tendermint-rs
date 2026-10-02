//! Tendermint 0.34 node identity and the secret-connection handshake.
//!
//! `NodeKey` is the peer private key in `node_key.json`. `SecretConnection` is the STS
//! handshake from `p2p/conn`: X25519 ephemeral keys, a Merlin transcript, HKDF-SHA256,
//! and ChaCha20-Poly1305 frames. There is no reactor, PEX, or dial loop.

mod error;
mod node_key;
mod secret_connection;

pub use error::Error;
pub use node_key::NodeKey;
pub use secret_connection::{SecretConnection, derive_secrets, make_secret_connection};
