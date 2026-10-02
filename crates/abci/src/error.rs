use std::fmt;
use std::io;

/// Failures from the ABCI socket client.
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Proto(String),
    /// Length is negative or above `maxMsgSize` (100 MiB). Go returns `io.ErrShortBuffer`.
    MessageTooBig {
        len: i64,
    },
    InvalidVarint,
    /// `ResponseException.error`.
    Exception(String),
    /// The response oneof is missing or is not the type this call sent.
    MismatchedResponse,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Proto(err) => write!(f, "invalid protobuf: {err}"),
            Self::MessageTooBig { len } => write!(f, "invalid message length {len}"),
            Self::InvalidVarint => write!(f, "invalid varint"),
            Self::Exception(err) => write!(f, "{err}"),
            Self::MismatchedResponse => write!(f, "unexpected response"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}
