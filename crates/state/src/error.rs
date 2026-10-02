use std::fmt;

use eld_tendermint_types::Error as TypesError;

/// Failures from genesis state, block validation, and validator updates.
#[derive(Debug)]
pub enum Error {
    Types(TypesError),
    WrongHeight {
        wanted: i64,
        got: i64,
    },
    WrongChainId {
        wanted: String,
        got: String,
    },
    WrongLastBlockId,
    WrongAppHash,
    WrongLastResultsHash,
    WrongValidatorsHash,
    WrongNextValidatorsHash,
    WrongConsensusHash,
    /// `last_commit` is missing after the initial height. Go rejects a nil commit
    /// in `ValidateBasic`; this task allows it only at `initial_height`.
    NilLastCommit,
    InitialCommitHasSignatures,
    WrongLastCommitHash,
    NegativeVotingPower,
    /// Genesis had no validators and `InitChain` did not return any.
    NilValidatorSet,
    UnsupportedPubKeyType {
        got: String,
    },
    MissingValidators,
    MissingNextValidators,
    MissingLastValidators,
    MissingConsensusParams,
    Db(String),
    /// The socket app returned an I/O or protocol error.
    Abci(String),
    /// `txindex.ErrorEmptyHash`.
    EmptyTxHash,
    /// A stored `TxResult` did not decode.
    BadTxResult(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Types(err) => write!(f, "{err}"),
            Self::WrongHeight { wanted, got } => {
                write!(f, "wrong Block.Header.Height. Expected {wanted}, got {got}")
            }
            Self::WrongChainId { wanted, got } => {
                write!(
                    f,
                    "wrong Block.Header.ChainID. Expected {wanted}, got {got}"
                )
            }
            Self::WrongLastBlockId => write!(f, "wrong Block.Header.LastBlockID"),
            Self::WrongAppHash => write!(f, "wrong Block.Header.AppHash"),
            Self::WrongLastResultsHash => write!(f, "wrong Block.Header.LastResultsHash"),
            Self::WrongValidatorsHash => write!(f, "wrong Block.Header.ValidatorsHash"),
            Self::WrongNextValidatorsHash => write!(f, "wrong Block.Header.NextValidatorsHash"),
            Self::WrongConsensusHash => write!(f, "wrong Block.Header.ConsensusHash"),
            Self::NilLastCommit => write!(f, "nil LastCommit"),
            Self::InitialCommitHasSignatures => {
                write!(f, "initial block can't have LastCommit signatures")
            }
            Self::WrongLastCommitHash => write!(f, "wrong Header.LastCommitHash"),
            Self::NegativeVotingPower => write!(f, "voting power can't be negative"),
            Self::NilValidatorSet => {
                write!(
                    f,
                    "validator set is nil in genesis and still empty after InitChain"
                )
            }
            Self::UnsupportedPubKeyType { got } => {
                write!(
                    f,
                    "validator is using pubkey {got}, which is unsupported for consensus"
                )
            }
            Self::MissingValidators => write!(f, "state is missing validators"),
            Self::MissingNextValidators => write!(f, "state is missing next validators"),
            Self::MissingLastValidators => write!(f, "state is missing last validators"),
            Self::MissingConsensusParams => write!(f, "state is missing consensus params"),
            Self::Db(message) => write!(f, "state db: {message}"),
            Self::Abci(message) => write!(f, "{message}"),
            Self::EmptyTxHash => write!(f, "transaction hash cannot be empty"),
            Self::BadTxResult(message) => write!(f, "error reading TxResult: {message}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Types(err) => Some(err),
            _ => None,
        }
    }
}

impl From<TypesError> for Error {
    fn from(err: TypesError) -> Self {
        Self::Types(err)
    }
}
