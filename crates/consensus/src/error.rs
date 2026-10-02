use std::fmt;

/// Failures from starting a validator, signing, or reading the WAL.
#[derive(Debug)]
pub enum Error {
    Mempool(eld_tendermint_mempool::Error),
    State(eld_tendermint_state::Error),
    Privval(eld_tendermint_privval::Error),
    NotValidator,
    /// A WAL record is truncated, too long, or its CRC or protobuf does not match.
    CorruptWal(String),
    Io(String),
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

impl std::error::Error for Error {}

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
