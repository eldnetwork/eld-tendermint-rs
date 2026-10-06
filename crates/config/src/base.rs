//! `config.BaseConfig`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::error::Error;
use crate::path::rootify;
use crate::{CONFIG_DIR, DATA_DIR, DEFAULT_LOG_LEVEL, LOG_FORMAT_JSON, LOG_FORMAT_PLAIN};

/// Top-level node options. `chain_id` is not stored here; it comes from genesis.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct BaseConfig {
    /// `home`. Set from `--home` / `TMHOME` after the file is read.
    #[serde(rename = "home", default)]
    pub root_dir: String,
    #[serde(default = "default_proxy_app")]
    pub proxy_app: String,
    #[serde(default = "crate::hostname::default_moniker")]
    pub moniker: String,
    #[serde(rename = "fast_sync", default = "crate::default_true")]
    pub fast_sync: bool,
    #[serde(default = "default_db_backend")]
    pub db_backend: String,
    #[serde(rename = "db_dir", default = "default_db_dir")]
    pub db_dir: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_log_format")]
    pub log_format: String,
    #[serde(default = "default_genesis_file")]
    pub genesis_file: String,
    #[serde(default = "default_priv_validator_key_file")]
    pub priv_validator_key_file: String,
    #[serde(default = "default_priv_validator_state_file")]
    pub priv_validator_state_file: String,
    #[serde(default)]
    pub priv_validator_laddr: String,
    #[serde(default = "default_node_key_file")]
    pub node_key_file: String,
    #[serde(default = "default_abci")]
    pub abci: String,
    #[serde(default)]
    pub filter_peers: bool,
}

impl BaseConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            root_dir: String::new(),
            proxy_app: default_proxy_app(),
            moniker: crate::hostname::default_moniker(),
            fast_sync: true,
            db_backend: default_db_backend(),
            db_dir: default_db_dir(),
            log_level: default_log_level(),
            log_format: default_log_format(),
            genesis_file: default_genesis_file(),
            priv_validator_key_file: default_priv_validator_key_file(),
            priv_validator_state_file: default_priv_validator_state_file(),
            priv_validator_laddr: String::new(),
            node_key_file: default_node_key_file(),
            abci: default_abci(),
            filter_peers: false,
        }
    }

    /// `TestBaseConfig`. The Go helper also sets an unexported chain id; that field is not loaded from TOML.
    #[must_use]
    pub fn test_config() -> Self {
        let mut cfg = Self::default_config();
        cfg.proxy_app = "kvstore".to_owned();
        cfg.fast_sync = false;
        cfg.db_backend = "memdb".to_owned();
        cfg
    }

    #[must_use]
    pub fn genesis_file(&self) -> PathBuf {
        rootify(&self.genesis_file, &self.root_dir)
    }

    #[must_use]
    pub fn priv_validator_key_file(&self) -> PathBuf {
        rootify(&self.priv_validator_key_file, &self.root_dir)
    }

    #[must_use]
    pub fn priv_validator_state_file(&self) -> PathBuf {
        rootify(&self.priv_validator_state_file, &self.root_dir)
    }

    #[must_use]
    pub fn node_key_file(&self) -> PathBuf {
        rootify(&self.node_key_file, &self.root_dir)
    }

    #[must_use]
    pub fn db_dir(&self) -> PathBuf {
        rootify(&self.db_dir, &self.root_dir)
    }

    /// `BaseConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownLogFormat`] when `log_format` is not `plain` or `json`.
    pub fn validate_basic(&self) -> Result<(), Error> {
        match self.log_format.as_str() {
            LOG_FORMAT_PLAIN | LOG_FORMAT_JSON => Ok(()),
            _ => Err(Error::UnknownLogFormat),
        }
    }
}

pub(crate) fn default_proxy_app() -> String {
    "tcp://127.0.0.1:26658".to_owned()
}

pub(crate) fn default_db_backend() -> String {
    "goleveldb".to_owned()
}

pub(crate) fn default_db_dir() -> String {
    DATA_DIR.to_owned()
}

pub(crate) fn default_log_level() -> String {
    DEFAULT_LOG_LEVEL.to_owned()
}

pub(crate) fn default_log_format() -> String {
    LOG_FORMAT_PLAIN.to_owned()
}

pub(crate) fn default_genesis_file() -> String {
    format!("{CONFIG_DIR}/genesis.json")
}

pub(crate) fn default_priv_validator_key_file() -> String {
    format!("{CONFIG_DIR}/priv_validator_key.json")
}

pub(crate) fn default_priv_validator_state_file() -> String {
    format!("{DATA_DIR}/priv_validator_state.json")
}

pub(crate) fn default_node_key_file() -> String {
    format!("{CONFIG_DIR}/node_key.json")
}

pub(crate) fn default_abci() -> String {
    "socket".to_owned()
}
