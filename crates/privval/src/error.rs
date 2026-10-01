use std::fmt;
use std::path::PathBuf;

use eld_tendermint_crypto::Error as CryptoError;

/// Failures from file privval load, save, and double-sign checks.
#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        message: String,
    },
    Json(String),
    Key(CryptoError),
    Crypto(CryptoError),
    HeightRegression {
        got: i64,
        last: i64,
    },
    RoundRegression {
        height: i64,
        got: i32,
        last: i32,
    },
    StepRegression {
        height: i64,
        round: i32,
        got: i8,
        last: i8,
    },
    /// HRS matches and sign bytes are present, but the stored signature is missing.
    /// Go panics in this case.
    MissingLastSignature,
    /// HRS matches, but the last state has no sign bytes.
    NoSignBytes,
    /// Same height, round, and step, with different sign bytes.
    ConflictingData,
    UnknownVoteType,
    /// Stored sign bytes are not a length-prefixed canonical vote or proposal.
    /// Go panics in this case.
    CorruptSignBytes,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "{message}: {}", path.display()),
            Self::Json(message) => write!(f, "invalid privval json: {message}"),
            Self::Key(err) => write!(f, "invalid privval key: {err}"),
            Self::Crypto(err) => write!(f, "signing failed: {err}"),
            Self::HeightRegression { got, last } => {
                write!(f, "height regression. Got {got}, last height {last}")
            }
            Self::RoundRegression { height, got, last } => write!(
                f,
                "round regression at height {height}. Got {got}, last round {last}"
            ),
            Self::StepRegression {
                height,
                round,
                got,
                last,
            } => write!(
                f,
                "step regression at height {height} round {round}. Got {got}, last step {last}"
            ),
            Self::MissingLastSignature => {
                write!(f, "pv: Signature is nil but SignBytes is not!")
            }
            Self::NoSignBytes => write!(f, "no SignBytes found"),
            Self::ConflictingData => write!(f, "conflicting data"),
            Self::UnknownVoteType => write!(f, "unknown vote type"),
            Self::CorruptSignBytes => write!(f, "stored sign bytes cannot be decoded"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Key(err) | Self::Crypto(err) => Some(err),
            _ => None,
        }
    }
}
