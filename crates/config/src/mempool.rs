//! `config.MempoolConfig`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::MEMPOOL_V0;
use crate::duration::Duration;
use crate::error::{Error, non_negative};
use crate::path::rootify;

/// Mempool options.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct MempoolConfig {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(rename = "home", default)]
    pub root_dir: String,
    #[serde(default = "crate::default_true")]
    pub recheck: bool,
    #[serde(default = "crate::default_true")]
    pub broadcast: bool,
    #[serde(default)]
    pub wal_dir: String,
    #[serde(default = "default_size")]
    pub size: i64,
    #[serde(default = "default_max_txs_bytes")]
    pub max_txs_bytes: i64,
    #[serde(default = "default_cache_size")]
    pub cache_size: i64,
    #[serde(rename = "keep-invalid-txs-in-cache", default)]
    pub keep_invalid_txs_in_cache: bool,
    #[serde(default = "default_max_tx_bytes")]
    pub max_tx_bytes: i64,
    #[serde(default)]
    pub max_batch_bytes: i64,
    #[serde(rename = "ttl-duration", default)]
    pub ttl_duration: Duration,
    #[serde(rename = "ttl-num-blocks", default)]
    pub ttl_num_blocks: i64,
}

impl MempoolConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            version: default_version(),
            root_dir: String::new(),
            recheck: true,
            broadcast: true,
            wal_dir: String::new(),
            size: default_size(),
            max_txs_bytes: default_max_txs_bytes(),
            cache_size: default_cache_size(),
            keep_invalid_txs_in_cache: false,
            max_tx_bytes: default_max_tx_bytes(),
            max_batch_bytes: 0,
            ttl_duration: Duration::default(),
            ttl_num_blocks: 0,
        }
    }

    /// `TestMempoolConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        let mut cfg = Self::default_config();
        cfg.cache_size = 1000;
        cfg
    }

    #[must_use]
    pub fn wal_dir(&self) -> PathBuf {
        rootify(&self.wal_dir, &self.root_dir)
    }

    #[must_use]
    pub fn wal_enabled(&self) -> bool {
        !self.wal_dir.is_empty()
    }

    /// `MempoolConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when a size or cache limit is negative.
    pub fn validate_basic(&self) -> Result<(), Error> {
        non_negative(self.size, "size")?;
        non_negative(self.max_txs_bytes, "max_txs_bytes")?;
        non_negative(self.cache_size, "cache_size")?;
        non_negative(self.max_tx_bytes, "max_tx_bytes")?;
        Ok(())
    }
}

fn default_version() -> String {
    MEMPOOL_V0.to_owned()
}

fn default_size() -> i64 {
    5000
}

fn default_max_txs_bytes() -> i64 {
    1024 * 1024 * 1024
}

fn default_cache_size() -> i64 {
    10_000
}

fn default_max_tx_bytes() -> i64 {
    1024 * 1024
}
