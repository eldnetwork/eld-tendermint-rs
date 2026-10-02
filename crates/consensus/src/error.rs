use std::fmt;

/// Failures from starting a validator or signing a vote outside the round.
#[derive(Debug)]
pub enum Error {
    Mempool(eld_tendermint_mempool::Error),
    State(eld_tendermint_state::Error),
    Privval(String),
    NotValidator,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mempool(err) => write!(f, "{err}"),
            Self::State(err) => write!(f, "{err}"),
            Self::Privval(err) => write!(f, "{err}"),
            Self::NotValidator => write!(f, "local key is not in the validator set"),
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
