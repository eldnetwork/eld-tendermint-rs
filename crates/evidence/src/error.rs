//! Evidence pool errors.

use std::fmt;

/// A vote that failed `ValidateBasic` or `VerifyDuplicateVote`, or a database write.
#[derive(Debug)]
pub enum Error {
    /// The evidence is not valid for the current or last validator set.
    Invalid(eld_tendermint_types::Error),
    /// `evidence.db` rejected a read or write.
    Db(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(err) => write!(f, "invalid evidence: {err}"),
            Self::Db(err) => write!(f, "evidence db: {err}"),
        }
    }
}

impl std::error::Error for Error {}
