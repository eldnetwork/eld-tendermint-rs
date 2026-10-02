use std::fmt;
use std::io;

use eld_tendermint_crypto::Error as CryptoError;

/// Failures from node-key files and the secret-connection handshake.
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
