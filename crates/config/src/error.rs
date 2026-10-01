//! Failures from TOML parsing, `ValidateBasic`, and genesis file reads.

use std::fmt;

use eld_tendermint_types::Error as GenesisError;

/// Config load and validation failures.
#[derive(Debug)]
pub enum Error {
    Io {
        path: String,
        source: std::io::Error,
    },
    Toml(String),
    InvalidDuration(String),
    UnknownLogFormat,
    Negative {
        field: &'static str,
    },
    SubscriptionBufferTooSmall {
        got: i64,
    },
    WebSocketBufferTooSmall {
        subscription: i64,
    },
    RpcServersRequired,
    RpcServersTooFew,
    EmptyRpcServer,
    DiscoveryTimeTooSmall,
    TrustPeriodRequired,
    TrustHeightRequired,
    TrustHashRequired,
    InvalidTrustHash,
    ChunkRequestTimeoutTooSmall,
    ChunkFetchersRequired,
    UnknownFastSyncVersion {
        got: String,
    },
    Section {
        section: &'static str,
        source: Box<Error>,
    },
    GenesisRead {
        path: String,
        source: std::io::Error,
    },
    Genesis {
        path: String,
        source: GenesisError,
    },
    ConfigNotFound {
        home: String,
    },
    HomeNotSet,
    Usage(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "failed to read {path}: {source}"),
            Self::Toml(msg) => write!(f, "invalid config toml: {msg}"),
            Self::InvalidDuration(text) => write!(f, "invalid duration: {text}"),
            Self::UnknownLogFormat => {
                write!(f, "unknown log_format (must be 'plain' or 'json')")
            }
            Self::Negative { field } => write!(f, "{field} can't be negative"),
            Self::SubscriptionBufferTooSmall { .. } => {
                write!(
                    f,
                    "experimental_subscription_buffer_size must be >= {}",
                    crate::MIN_SUBSCRIPTION_BUFFER_SIZE
                )
            }
            Self::WebSocketBufferTooSmall { subscription } => {
                write!(
                    f,
                    "experimental_websocket_write_buffer_size must be >= experimental_subscription_buffer_size ({subscription})"
                )
            }
            Self::RpcServersRequired => write!(f, "rpc_servers is required"),
            Self::RpcServersTooFew => {
                write!(f, "at least two rpc_servers entries is required")
            }
            Self::EmptyRpcServer => write!(f, "found empty rpc_servers entry"),
            Self::DiscoveryTimeTooSmall => {
                write!(f, "discovery time must be 0s or greater than five seconds")
            }
            Self::TrustPeriodRequired => write!(f, "trusted_period is required"),
            Self::TrustHeightRequired => write!(f, "trusted_height is required"),
            Self::TrustHashRequired => write!(f, "trusted_hash is required"),
            Self::InvalidTrustHash => write!(f, "invalid trusted_hash"),
            Self::ChunkRequestTimeoutTooSmall => {
                write!(f, "chunk_request_timeout must be at least 5 seconds")
            }
            Self::ChunkFetchersRequired => write!(f, "chunk_fetchers is required"),
            Self::UnknownFastSyncVersion { got } => {
                write!(f, "unknown fastsync version {got}")
            }
            Self::Section { section, source } => {
                write!(f, "error in [{section}] section: {source}")
            }
            Self::GenesisRead { path, source } => {
                write!(f, "couldn't read GenesisDoc file {path}: {source}")
            }
            Self::Genesis { path, source } => {
                write!(f, "error reading GenesisDoc at {path}: {source}")
            }
            Self::ConfigNotFound { home } => {
                write!(f, "config.toml not found in {home} or {home}/config")
            }
            Self::HomeNotSet => write!(f, "HOME is not set"),
            Self::Usage(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::GenesisRead { source, .. } => Some(source),
            Self::Section { source, .. } => Some(source),
            Self::Genesis { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn non_negative(value: i64, field: &'static str) -> Result<(), Error> {
    if value < 0 {
        Err(Error::Negative { field })
    } else {
        Ok(())
    }
}

pub(crate) fn in_section(section: &'static str, result: Result<(), Error>) -> Result<(), Error> {
    result.map_err(|err| Error::Section {
        section,
        source: Box::new(err),
    })
}
