use std::fmt;
use std::io;

use eld_tendermint_crypto::Error as CryptoError;

/// Failures from node-key files, the secret connection, and the p2p switch.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Json(String),
    Crypto(CryptoError),
    Proto(String),
    /// `curve25519.X25519` rejected the remote ephemeral key.
    LowOrderRemotePubKey,
    /// Remote signature over the Merlin challenge did not verify.
    ChallengeVerification,
    InvalidVarint,
    MessageTooBig {
        len: u64,
    },
    ChunkTooBig,
    Decrypt,
    /// Go panics when the ChaCha20-Poly1305 nonce would wrap.
    NonceOverflow,
    /// `pongTimeout` must be less than `pingInterval`.
    BadPingConfig,
    /// Reassembled `PacketMsg` bytes exceed the channel `RecvMessageCapacity`.
    MessageExceedsCapacity {
        cap: usize,
        got: usize,
    },
    /// Two reactors registered the same channel id.
    DuplicateChannel {
        id: u8,
    },
    /// Channel priority or send-queue capacity is zero.
    InvalidChannel,
    /// Inbound `PacketMsg` channel is not registered on this connection.
    UnknownChannel {
        id: i32,
    },
    /// Peer did not answer `PacketPing` before `pongTimeout`.
    PongTimeout,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "invalid node key json: {err}"),
            Self::Crypto(err) => write!(f, "{err}"),
            Self::Proto(err) => write!(f, "invalid protobuf: {err}"),
            Self::LowOrderRemotePubKey => {
                write!(f, "detected low order point from remote peer")
            }
            Self::ChallengeVerification => write!(f, "challenge verification failed"),
            Self::InvalidVarint => write!(f, "invalid varint"),
            Self::MessageTooBig { len } => write!(f, "message length {len} exceeds max"),
            Self::ChunkTooBig => write!(f, "chunkLength is greater than dataMaxSize"),
            Self::Decrypt => write!(f, "failed to decrypt SecretConnection"),
            Self::NonceOverflow => write!(f, "can't increase nonce without overflow"),
            Self::BadPingConfig => {
                write!(f, "pongTimeout must be less than pingInterval")
            }
            Self::MessageExceedsCapacity { cap, got } => {
                write!(
                    f,
                    "received message exceeds available capacity: {cap} < {got}"
                )
            }
            Self::DuplicateChannel { id } => write!(f, "channel {id:X} has multiple reactors"),
            Self::InvalidChannel => {
                write!(
                    f,
                    "channel priority and send queue capacity must be positive"
                )
            }
            Self::UnknownChannel { id } => write!(f, "unknown channel {id:X}"),
            Self::PongTimeout => write!(f, "pong timeout"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Crypto(err) => Some(err),
            _ => None,
        }
    }
}

impl Error {
    pub(crate) fn into_io(self) -> io::Error {
        match self {
            Self::Io(err) => err,
            other => io::Error::new(io::ErrorKind::InvalidData, other),
        }
    }
}
