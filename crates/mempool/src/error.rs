use std::fmt;

/// Failures from `CheckTx` before the transaction is accepted.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// `ErrMempoolIsFull`.
    MempoolIsFull {
        num_txs: i64,
        max_txs: i64,
        txs_bytes: i64,
        max_txs_bytes: i64,
    },
    /// `ErrTxTooLarge`.
    TxTooLarge { max: i64, actual: i64 },
    /// `ErrTxInCache`.
    TxInCache,
    /// `ErrPreCheck`. The display text is the reason alone, as in Go.
    PreCheck { reason: String },
    /// `Update` was given a different number of txs and deliver responses.
    /// Go indexes the slices and panics. This is that programmer error as `Result`.
    MismatchedDeliverResponses { txs: usize, responses: usize },
    /// This crate is the v0 FIFO mempool.
    UnsupportedVersion { got: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MempoolIsFull {
                num_txs,
                max_txs,
                txs_bytes,
                max_txs_bytes,
            } => write!(
                f,
                "mempool is full: number of txs {num_txs} (max: {max_txs}), total txs bytes {txs_bytes} (max: {max_txs_bytes})"
            ),
            Self::TxTooLarge { max, actual } => {
                write!(f, "Tx too large. Max size is {max}, but got {actual}")
            }
            Self::TxInCache => write!(f, "tx already exists in cache"),
            Self::PreCheck { reason } => write!(f, "{reason}"),
            Self::MismatchedDeliverResponses { txs, responses } => {
                write!(f, "deliver responses {responses} != committed txs {txs}")
            }
            Self::UnsupportedVersion { got } => {
                write!(f, "mempool version {got} is not v0")
            }
        }
    }
}

impl std::error::Error for Error {}
