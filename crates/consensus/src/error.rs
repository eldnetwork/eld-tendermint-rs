use std::fmt;
use std::io;

/// Why a WAL record cannot be replayed. The text matches Go `DataCorruptionError`.
#[derive(Debug)]
pub enum WalCorrupt {
    EmptyMessage,
    EmptyMsgInfo,
    MissingProposal,
    MissingVote,
    /// A proposal, part, or vote failed `try_from_proto`.
    Decode(String),
    MsgTooBig {
        length: u32,
        max: u32,
    },
    MissingLength,
    LengthTooBig {
        length: u32,
        max: u32,
    },
    ShortRead(io::Error),
    Checksum {
        read: u32,
        actual: u32,
    },
    Proto(prost::DecodeError),
    ShortChecksum,
    Io(io::Error),
}

impl fmt::Display for WalCorrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyMessage => write!(f, "empty WAL message"),
            Self::EmptyMsgInfo => write!(f, "empty msg info"),
            Self::MissingProposal => write!(f, "missing proposal"),
            Self::MissingVote => write!(f, "missing vote"),
            Self::Decode(err) => write!(f, "{err}"),
            Self::MsgTooBig { length, max } => {
                write!(f, "msg is too big: {length} bytes, max: {max} bytes")
            }
            Self::MissingLength => write!(f, "failed to read length"),
            Self::LengthTooBig { length, max } => {
                write!(
                    f,
                    "length {length} exceeded maximum possible value of {max} bytes"
                )
            }
            Self::ShortRead(err) => write!(f, "failed to read data: {err}"),
            Self::Checksum { read, actual } => {
                write!(f, "checksums do not match: read: {read}, actual: {actual}")
            }
            Self::Proto(err) => write!(f, "failed to decode data: {err}"),
            Self::ShortChecksum => write!(f, "failed to read checksum"),
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for WalCorrupt {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ShortRead(err) | Self::Io(err) => Some(err),
            Self::Proto(err) => Some(err),
            _ => None,
        }
    }
}

/// Failures from starting a validator, signing, or reading the WAL.
#[derive(Debug)]
pub enum Error {
    Mempool(eld_tendermint_mempool::Error),
    State(eld_tendermint_state::Error),
    Privval(eld_tendermint_privval::Error),
    NotValidator,
    /// A WAL record is truncated, too long, or its CRC or protobuf does not match.
    CorruptWal(WalCorrupt),
    Io(io::Error),
    /// Replay was asked for a height whose `EndHeight` marker is not in the WAL.
    MissingEndHeight {
        height: i64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mempool(err) => write!(f, "{err}"),
            Self::State(err) => write!(f, "{err}"),
            Self::Privval(err) => write!(f, "{err}"),
            Self::NotValidator => write!(f, "local key is not in the validator set"),
            Self::CorruptWal(err) => write!(f, "DataCorruptionError[{err}]"),
            Self::Io(err) => write!(f, "{err}"),
            Self::MissingEndHeight { height } => {
                write!(
                    f,
                    "cannot replay height. WAL does not contain #ENDHEIGHT for {height}"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Mempool(err) => Some(err),
            Self::State(err) => Some(err),
            Self::Privval(err) => Some(err),
            Self::CorruptWal(err) => Some(err),
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<eld_tendermint_mempool::Error> for Error {
    fn from(err: eld_tendermint_mempool::Error) -> Self {
        Self::Mempool(err)
    }
}

impl From<eld_tendermint_state::Error> for Error {
    fn from(err: eld_tendermint_state::Error) -> Self {
        Self::State(err)
    }
}

impl From<eld_tendermint_privval::Error> for Error {
    fn from(err: eld_tendermint_privval::Error) -> Self {
        Self::Privval(err)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}
