//! Tendermint 0.34 `config.toml` and genesis file loader.
//!
//! Parsing starts from the Go defaults and overlays the file. Missing keys keep those
//! defaults. `chain_id` comes from genesis, not from TOML.

mod base;
mod consensus;
mod duration;
mod error;
mod fastsync;
mod instrumentation;
mod load;
mod mempool;
mod p2p;
mod path;
mod rpc;
mod statesync;
mod storage;
mod tx_index;

use std::path::{Path, PathBuf};

use eld_tendermint_types::GenesisDoc;
use serde::Deserialize;

pub use base::{BaseConfig, default_moniker};
pub use consensus::ConsensusConfig;
pub use duration::Duration;
pub use error::Error;
pub use fastsync::FastSyncConfig;
pub use instrumentation::InstrumentationConfig;
pub use load::{find_config_file, load_file, load_home, load_toml, resolve_home};
pub use mempool::MempoolConfig;
pub use p2p::{FuzzConnConfig, P2pConfig};
pub use rpc::RpcConfig;
pub use statesync::StateSyncConfig;
pub use storage::StorageConfig;
pub use tx_index::TxIndexConfig;

use crate::error::in_section;

/// Directory name under `$HOME` when `--home` and `TMHOME` are unset.
pub const DEFAULT_TENDERMINT_DIR: &str = ".tendermint";

/// `config` directory inside the node home.
pub const CONFIG_DIR: &str = "config";

/// `data` directory inside the node home.
pub const DATA_DIR: &str = "data";

/// `log_format = "plain"`.
pub const LOG_FORMAT_PLAIN: &str = "plain";

/// `log_format = "json"`.
pub const LOG_FORMAT_JSON: &str = "json";

/// `DefaultLogLevel`.
pub const DEFAULT_LOG_LEVEL: &str = "info";

/// FIFO mempool.
pub const MEMPOOL_V0: &str = "v0";

/// Prioritized mempool.
pub const MEMPOOL_V1: &str = "v1";

/// `FuzzModeDrop`.
pub const FUZZ_MODE_DROP: i64 = 0;

/// `FuzzModeDelay`.
pub const FUZZ_MODE_DELAY: i64 = 1;

/// Minimum `experimental_subscription_buffer_size`.
pub const MIN_SUBSCRIPTION_BUFFER_SIZE: i64 = 100;

/// Default subscription and websocket buffer size.
pub const DEFAULT_SUBSCRIPTION_BUFFER_SIZE: i64 = 200;

pub(crate) fn default_true() -> bool {
    true
}

/// Top-level Tendermint node configuration (`config.Config`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Config {
    #[serde(flatten)]
    pub base: BaseConfig,
    #[serde(default = "RpcConfig::default_config")]
    pub rpc: RpcConfig,
    #[serde(default = "P2pConfig::default_config")]
    pub p2p: P2pConfig,
    #[serde(default = "MempoolConfig::default_config")]
    pub mempool: MempoolConfig,
    #[serde(default = "StateSyncConfig::default_config")]
    pub statesync: StateSyncConfig,
    #[serde(default = "FastSyncConfig::default_config")]
    pub fastsync: FastSyncConfig,
    #[serde(default = "ConsensusConfig::default_config")]
    pub consensus: ConsensusConfig,
    #[serde(default = "StorageConfig::default_config")]
    pub storage: StorageConfig,
    #[serde(rename = "tx_index", default = "TxIndexConfig::default_config")]
    pub tx_index: TxIndexConfig,
    #[serde(default = "InstrumentationConfig::default_config")]
    pub instrumentation: InstrumentationConfig,
}

impl Config {
    /// `DefaultConfig`.
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            base: BaseConfig::default_config(),
            rpc: RpcConfig::default_config(),
            p2p: P2pConfig::default_config(),
            mempool: MempoolConfig::default_config(),
            statesync: StateSyncConfig::default_config(),
            fastsync: FastSyncConfig::default_config(),
            consensus: ConsensusConfig::default_config(),
            storage: StorageConfig::default_config(),
            tx_index: TxIndexConfig::default_config(),
            instrumentation: InstrumentationConfig::default_config(),
        }
    }

    /// `TestConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self {
            base: BaseConfig::test_config(),
            rpc: RpcConfig::test_config(),
            p2p: P2pConfig::test_config(),
            mempool: MempoolConfig::test_config(),
            statesync: StateSyncConfig::test_config(),
            fastsync: FastSyncConfig::test_config(),
            consensus: ConsensusConfig::test_config(),
            storage: StorageConfig::test_config(),
            tx_index: TxIndexConfig::test_config(),
            instrumentation: InstrumentationConfig::test_config(),
        }
    }

    /// `Config.SetRoot`.
    pub fn set_root(&mut self, root: impl AsRef<Path>) {
        let root = root.as_ref().to_string_lossy().into_owned();
        self.base.root_dir.clone_from(&root);
        self.rpc.root_dir.clone_from(&root);
        self.p2p.root_dir.clone_from(&root);
        self.mempool.root_dir.clone_from(&root);
        self.consensus.root_dir = root;
    }

    #[must_use]
    pub fn genesis_file(&self) -> PathBuf {
        self.base.genesis_file()
    }

    #[must_use]
    pub fn priv_validator_key_file(&self) -> PathBuf {
        self.base.priv_validator_key_file()
    }

    #[must_use]
    pub fn priv_validator_state_file(&self) -> PathBuf {
        self.base.priv_validator_state_file()
    }

    #[must_use]
    pub fn node_key_file(&self) -> PathBuf {
        self.base.node_key_file()
    }

    #[must_use]
    pub fn db_dir(&self) -> PathBuf {
        self.base.db_dir()
    }

    /// `Config.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns the base error as-is, or `error in [section] section: ...` for a section check.
    pub fn validate_basic(&self) -> Result<(), Error> {
        self.base.validate_basic()?;
        in_section("rpc", self.rpc.validate_basic())?;
        in_section("p2p", self.p2p.validate_basic())?;
        in_section("mempool", self.mempool.validate_basic())?;
        in_section("statesync", self.statesync.validate_basic())?;
        in_section("fastsync", self.fastsync.validate_basic())?;
        in_section("consensus", self.consensus.validate_basic())?;
        in_section("instrumentation", self.instrumentation.validate_basic())?;
        Ok(())
    }

    /// Read [`Self::genesis_file`] and parse it with [`GenesisDoc::from_json`].
    ///
    /// # Errors
    ///
    /// Returns an I/O error or a genesis validation error.
    pub fn load_genesis(&self) -> Result<GenesisDoc, Error> {
        let path = self.genesis_file();
        let bytes = std::fs::read(&path).map_err(|source| Error::GenesisRead {
            path: path.display().to_string(),
            source,
        })?;
        GenesisDoc::from_json(&bytes).map_err(|source| Error::Genesis {
            path: path.display().to_string(),
            source,
        })
    }
}
