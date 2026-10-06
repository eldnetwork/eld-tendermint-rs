use std::fmt;

/// Why a fast-sync block was not applied. The switch drops the peer that sent it.
/// The node process does not see this value. It keeps its own string error.
#[derive(Debug)]
pub enum Error {
    /// `last_commit` does not match the previous block.
    BadCommit,
    /// `validate_block` rejected the block.
    InvalidBlock(eld_tendermint_state::Error),
    /// The block could not be split into parts.
    PartSet(eld_tendermint_types::Error),
    /// The header hash is missing.
    MissingHash,
    /// `ApplyBlock` failed.
    Apply(eld_tendermint_state::Error),
    /// The block store rejected the save.
    Store(eld_tendermint_store::Error),
    /// The transaction index rejected the commit.
    TxIndex(eld_tendermint_state::Error),
    /// The state store rejected the save.
    State(eld_tendermint_state::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadCommit => write!(f, "commit does not match the previous block"),
            Self::InvalidBlock(err) => write!(f, "{err}"),
            Self::PartSet(err) => write!(f, "{err}"),
            Self::MissingHash => write!(f, "block hash is missing"),
            Self::Apply(err) => write!(f, "{err}"),
            Self::Store(err) => write!(f, "{err}"),
            Self::TxIndex(err) => write!(f, "{err}"),
            Self::State(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidBlock(err) | Self::Apply(err) | Self::TxIndex(err) | Self::State(err) => {
                Some(err)
            }
            Self::PartSet(err) => Some(err),
            Self::Store(err) => Some(err),
            Self::BadCommit | Self::MissingHash => None,
        }
    }
}
