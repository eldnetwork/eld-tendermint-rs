use std::fmt;
use std::path::PathBuf;

/// Failures from `save_block` and from opening a database.
///
/// A value that is present but cannot be decoded is on-disk corruption. `BlockStore`
/// panics in that case, with the key in the message, matching Go. Database get and set
/// failures inside `BlockStore` panic the same way.
#[derive(Debug)]
pub enum Error {
    /// `PartSet.IsComplete` is false. Go panics with this condition.
    IncompletePartSet,
    /// `base > 0` and the block height is not `height + 1`. Go panics with this condition.
    NonContiguous {
        wanted: i64,
        got: i64,
    },
    /// The path is a Go goleveldb directory. RocksDB must not open it.
    GoLevelDb {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        message: String,
    },
    Db(String),
    /// `PruneBlocks` was asked for a height that is not positive.
    PruneHeight,
    /// `PruneBlocks` was asked to pass the latest saved height.
    BeyondLatest {
        height: i64,
        latest: i64,
    },
    /// `PruneBlocks` was asked for a height below the current base.
    BelowBase {
        height: i64,
        base: i64,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompletePartSet => {
                write!(f, "BlockStore can only save complete block part sets")
            }
            Self::NonContiguous { wanted, got } => write!(
                f,
                "BlockStore can only save contiguous blocks. Wanted {wanted}, got {got}"
            ),
            Self::GoLevelDb { path } => write!(
                f,
                "refusing to open goleveldb directory at {}",
                path.display()
            ),
            Self::Io { path, message } => write!(f, "{message}: {}", path.display()),
            Self::Db(message) => write!(f, "block store db: {message}"),
            Self::PruneHeight => write!(f, "height must be greater than 0"),
            Self::BeyondLatest { latest, .. } => {
                write!(f, "cannot prune beyond the latest height {latest}")
            }
            Self::BelowBase { height, base } => write!(
                f,
                "cannot prune to height {height}, it is lower than base height {base}"
            ),
        }
    }
}

impl std::error::Error for Error {}
