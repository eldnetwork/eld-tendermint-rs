//! Tendermint 0.34 node identity, the secret-connection handshake, and the p2p switch.
//!
//! `NodeKey` is the peer private key in `node_key.json`. `SecretConnection` is the STS
//! handshake from `p2p/conn`: X25519 ephemeral keys, a Merlin transcript, HKDF-SHA256,
//! and ChaCha20-Poly1305 frames. `MConnection` multiplexes length-delimited `Packet`s
//! on that stream. `Switch` dials `persistent_peers`, accepts TCP, and registers reactors.
//! `AddrBook` persists `addrbook.json`. [`pex::Reactor`] exchanges `PexRequest` and `PexAddrs`.

mod addrbook;
mod address;
mod connection;
mod ensured;
mod error;
mod node_key;
mod pex;
mod secret_connection;
mod switch;

pub use addrbook::{AddrBook, KnownAddress};
pub use address::{NetAddress, parse_persistent_peers};
pub use connection::{ChannelDescriptor, MConnConfig, MConnection};
pub use error::Error;
pub use node_key::NodeKey;
pub use pex::{
    MAX_GET_SELECTION, PEX_CHANNEL, Reactor as PexReactor,
    channel_descriptors as pex_channel_descriptors,
};
pub use secret_connection::{
    IoShutdown, SecretConnection, SecretReader, SecretWriter, SplitIo, derive_secrets,
    make_secret_connection,
};
pub use switch::{PeerInfo, Switch};
