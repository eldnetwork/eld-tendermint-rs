//! `config.ConsensusConfig`.

use std::path::PathBuf;

use eld_tendermint_types::Time;
use serde::Deserialize;

use crate::DATA_DIR;
use crate::duration::Duration;
use crate::error::{Error, non_negative};
use crate::path::rootify;

/// Consensus timeouts and WAL path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ConsensusConfig {
    #[serde(rename = "home", default)]
    pub root_dir: String,
    #[serde(default = "default_wal_file")]
    pub wal_file: String,
    /// `SetWalFile` override. Not read from TOML.
    #[serde(skip)]
    wal_file_override: String,
    #[serde(default = "default_timeout_propose")]
    pub timeout_propose: Duration,
    #[serde(default = "default_timeout_propose_delta")]
    pub timeout_propose_delta: Duration,
    #[serde(default = "default_timeout_prevote")]
    pub timeout_prevote: Duration,
    #[serde(default = "default_timeout_prevote_delta")]
    pub timeout_prevote_delta: Duration,
    #[serde(default = "default_timeout_precommit")]
    pub timeout_precommit: Duration,
    #[serde(default = "default_timeout_precommit_delta")]
    pub timeout_precommit_delta: Duration,
    #[serde(default = "default_timeout_commit")]
    pub timeout_commit: Duration,
    #[serde(default)]
    pub skip_timeout_commit: bool,
    #[serde(default = "crate::default_true")]
    pub create_empty_blocks: bool,
    #[serde(default)]
    pub create_empty_blocks_interval: Duration,
    #[serde(default = "default_peer_gossip_sleep_duration")]
    pub peer_gossip_sleep_duration: Duration,
    #[serde(default = "default_peer_query_maj23_sleep_duration")]
    pub peer_query_maj23_sleep_duration: Duration,
    #[serde(default)]
    pub double_sign_check_height: i64,
}

impl ConsensusConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            root_dir: String::new(),
            wal_file: default_wal_file(),
            wal_file_override: String::new(),
            timeout_propose: default_timeout_propose(),
            timeout_propose_delta: default_timeout_propose_delta(),
            timeout_prevote: default_timeout_prevote(),
            timeout_prevote_delta: default_timeout_prevote_delta(),
            timeout_precommit: default_timeout_precommit(),
            timeout_precommit_delta: default_timeout_precommit_delta(),
            timeout_commit: default_timeout_commit(),
            skip_timeout_commit: false,
            create_empty_blocks: true,
            create_empty_blocks_interval: Duration::default(),
            peer_gossip_sleep_duration: default_peer_gossip_sleep_duration(),
            peer_query_maj23_sleep_duration: default_peer_query_maj23_sleep_duration(),
            double_sign_check_height: 0,
        }
    }

    /// `TestConsensusConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        let mut cfg = Self::default_config();
        cfg.timeout_propose = Duration::from_millis(40);
        cfg.timeout_propose_delta = Duration::from_millis(1);
        cfg.timeout_prevote = Duration::from_millis(10);
        cfg.timeout_prevote_delta = Duration::from_millis(1);
        cfg.timeout_precommit = Duration::from_millis(10);
        cfg.timeout_precommit_delta = Duration::from_millis(1);
        cfg.timeout_commit = Duration::from_millis(10);
        cfg.skip_timeout_commit = true;
        cfg.peer_gossip_sleep_duration = Duration::from_millis(5);
        cfg.peer_query_maj23_sleep_duration = Duration::from_millis(250);
        cfg.double_sign_check_height = 0;
        cfg
    }

    #[must_use]
    pub fn wait_for_txs(&self) -> bool {
        !self.create_empty_blocks || self.create_empty_blocks_interval.as_nanos() > 0
    }

    /// `ConsensusConfig.Propose`.
    #[must_use]
    pub fn propose(&self, round: i32) -> Duration {
        round_timeout(self.timeout_propose, self.timeout_propose_delta, round)
    }

    /// `ConsensusConfig.Prevote`.
    #[must_use]
    pub fn prevote(&self, round: i32) -> Duration {
        round_timeout(self.timeout_prevote, self.timeout_prevote_delta, round)
    }

    /// `ConsensusConfig.Precommit`.
    #[must_use]
    pub fn precommit(&self, round: i32) -> Duration {
        round_timeout(self.timeout_precommit, self.timeout_precommit_delta, round)
    }

    /// `ConsensusConfig.Commit`: `time + timeout_commit`.
    #[must_use]
    pub fn commit(&self, time: Time) -> Time {
        let total = i128::from(time.unix_seconds()) * 1_000_000_000
            + i128::from(time.nanos())
            + i128::from(self.timeout_commit.as_nanos());
        let seconds = total.div_euclid(1_000_000_000);
        let nanos = total.rem_euclid(1_000_000_000);
        let seconds =
            i64::try_from(seconds).unwrap_or(if seconds < 0 { i64::MIN } else { i64::MAX });
        let nanos = i32::try_from(nanos).unwrap_or(0);
        Time::from_unix_parts(seconds, nanos)
    }

    #[must_use]
    pub fn wal_file(&self) -> PathBuf {
        if !self.wal_file_override.is_empty() {
            return PathBuf::from(&self.wal_file_override);
        }
        rootify(&self.wal_file, &self.root_dir)
    }

    pub fn set_wal_file(&mut self, wal_file: impl Into<String>) {
        self.wal_file_override = wal_file.into();
    }

    /// `ConsensusConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when a timeout or `double_sign_check_height` is negative.
    pub fn validate_basic(&self) -> Result<(), Error> {
        non_negative(self.timeout_propose.as_nanos(), "timeout_propose")?;
        non_negative(
            self.timeout_propose_delta.as_nanos(),
            "timeout_propose_delta",
        )?;
        non_negative(self.timeout_prevote.as_nanos(), "timeout_prevote")?;
        non_negative(
            self.timeout_prevote_delta.as_nanos(),
            "timeout_prevote_delta",
        )?;
        non_negative(self.timeout_precommit.as_nanos(), "timeout_precommit")?;
        non_negative(
            self.timeout_precommit_delta.as_nanos(),
            "timeout_precommit_delta",
        )?;
        non_negative(self.timeout_commit.as_nanos(), "timeout_commit")?;
        non_negative(
            self.create_empty_blocks_interval.as_nanos(),
            "create_empty_blocks_interval",
        )?;
        non_negative(
            self.peer_gossip_sleep_duration.as_nanos(),
            "peer_gossip_sleep_duration",
        )?;
        non_negative(
            self.peer_query_maj23_sleep_duration.as_nanos(),
            "peer_query_maj23_sleep_duration",
        )?;
        non_negative(self.double_sign_check_height, "double_sign_check_height")?;
        Ok(())
    }
}

fn round_timeout(base: Duration, delta: Duration, round: i32) -> Duration {
    Duration::from_nanos(
        base.as_nanos()
            .wrapping_add(delta.as_nanos().wrapping_mul(i64::from(round))),
    )
}

fn default_wal_file() -> String {
    format!("{DATA_DIR}/cs.wal/wal")
}

fn default_timeout_propose() -> Duration {
    Duration::from_secs(3)
}

fn default_timeout_propose_delta() -> Duration {
    Duration::from_millis(500)
}

fn default_timeout_prevote() -> Duration {
    Duration::from_secs(1)
}

fn default_timeout_prevote_delta() -> Duration {
    Duration::from_millis(500)
}

fn default_timeout_precommit() -> Duration {
    Duration::from_secs(1)
}

fn default_timeout_precommit_delta() -> Duration {
    Duration::from_millis(500)
}

fn default_timeout_commit() -> Duration {
    Duration::from_secs(1)
}

fn default_peer_gossip_sleep_duration() -> Duration {
    Duration::from_millis(100)
}

fn default_peer_query_maj23_sleep_duration() -> Duration {
    Duration::from_secs(2)
}
