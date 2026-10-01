//! `config.P2PConfig` and `FuzzConnConfig`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::duration::Duration;
use crate::error::{Error, non_negative};
use crate::path::rootify;
use crate::{CONFIG_DIR, FUZZ_MODE_DROP};

/// Fuzzed-connection options. The Go struct has no `mapstructure` tags, so the keys are the field names.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FuzzConnConfig {
    #[serde(rename = "Mode", default = "default_fuzz_mode")]
    pub mode: i64,
    #[serde(rename = "MaxDelay", default = "default_fuzz_max_delay")]
    pub max_delay: Duration,
    #[serde(rename = "ProbDropRW", default = "default_prob_drop_rw")]
    pub prob_drop_rw: f64,
    #[serde(rename = "ProbDropConn", default)]
    pub prob_drop_conn: f64,
    #[serde(rename = "ProbSleep", default)]
    pub prob_sleep: f64,
}

impl FuzzConnConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            mode: default_fuzz_mode(),
            max_delay: default_fuzz_max_delay(),
            prob_drop_rw: default_prob_drop_rw(),
            prob_drop_conn: 0.0,
            prob_sleep: 0.0,
        }
    }
}

fn default_fuzz_mode() -> i64 {
    FUZZ_MODE_DROP
}

fn default_fuzz_max_delay() -> Duration {
    Duration::from_secs(3)
}

fn default_prob_drop_rw() -> f64 {
    0.2
}

/// Peer-to-peer options.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct P2pConfig {
    #[serde(rename = "home", default)]
    pub root_dir: String,
    #[serde(default = "default_laddr")]
    pub laddr: String,
    #[serde(default)]
    pub external_address: String,
    #[serde(default)]
    pub seeds: String,
    #[serde(default)]
    pub persistent_peers: String,
    #[serde(default)]
    pub upnp: bool,
    #[serde(default = "default_addr_book_file")]
    pub addr_book_file: String,
    #[serde(default = "crate::default_true")]
    pub addr_book_strict: bool,
    #[serde(default = "default_max_num_inbound_peers")]
    pub max_num_inbound_peers: i64,
    #[serde(default = "default_max_num_outbound_peers")]
    pub max_num_outbound_peers: i64,
    #[serde(default)]
    pub unconditional_peer_ids: String,
    #[serde(default)]
    pub persistent_peers_max_dial_period: Duration,
    #[serde(default = "default_flush_throttle_timeout")]
    pub flush_throttle_timeout: Duration,
    #[serde(default = "default_max_packet_msg_payload_size")]
    pub max_packet_msg_payload_size: i64,
    #[serde(default = "default_send_rate")]
    pub send_rate: i64,
    #[serde(default = "default_recv_rate")]
    pub recv_rate: i64,
    #[serde(default = "crate::default_true")]
    pub pex: bool,
    #[serde(default)]
    pub seed_mode: bool,
    #[serde(default)]
    pub private_peer_ids: String,
    #[serde(default)]
    pub allow_duplicate_ip: bool,
    #[serde(default = "default_handshake_timeout")]
    pub handshake_timeout: Duration,
    #[serde(default = "default_dial_timeout")]
    pub dial_timeout: Duration,
    #[serde(default)]
    pub test_dial_fail: bool,
    #[serde(default)]
    pub test_fuzz: bool,
    #[serde(default = "FuzzConnConfig::default_config")]
    pub test_fuzz_config: FuzzConnConfig,
}

impl P2pConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            root_dir: String::new(),
            laddr: default_laddr(),
            external_address: String::new(),
            seeds: String::new(),
            persistent_peers: String::new(),
            upnp: false,
            addr_book_file: default_addr_book_file(),
            addr_book_strict: true,
            max_num_inbound_peers: default_max_num_inbound_peers(),
            max_num_outbound_peers: default_max_num_outbound_peers(),
            unconditional_peer_ids: String::new(),
            persistent_peers_max_dial_period: Duration::default(),
            flush_throttle_timeout: default_flush_throttle_timeout(),
            max_packet_msg_payload_size: default_max_packet_msg_payload_size(),
            send_rate: default_send_rate(),
            recv_rate: default_recv_rate(),
            pex: true,
            seed_mode: false,
            private_peer_ids: String::new(),
            allow_duplicate_ip: false,
            handshake_timeout: default_handshake_timeout(),
            dial_timeout: default_dial_timeout(),
            test_dial_fail: false,
            test_fuzz: false,
            test_fuzz_config: FuzzConnConfig::default_config(),
        }
    }

    /// `TestP2PConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        let mut cfg = Self::default_config();
        cfg.laddr = "tcp://127.0.0.1:36656".to_owned();
        cfg.flush_throttle_timeout = Duration::from_millis(10);
        cfg.allow_duplicate_ip = true;
        cfg
    }

    #[must_use]
    pub fn addr_book_file(&self) -> PathBuf {
        rootify(&self.addr_book_file, &self.root_dir)
    }

    /// `P2PConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when a peer limit, timeout, or rate is negative.
    pub fn validate_basic(&self) -> Result<(), Error> {
        non_negative(self.max_num_inbound_peers, "max_num_inbound_peers")?;
        non_negative(self.max_num_outbound_peers, "max_num_outbound_peers")?;
        non_negative(
            self.flush_throttle_timeout.as_nanos(),
            "flush_throttle_timeout",
        )?;
        non_negative(
            self.persistent_peers_max_dial_period.as_nanos(),
            "persistent_peers_max_dial_period",
        )?;
        non_negative(
            self.max_packet_msg_payload_size,
            "max_packet_msg_payload_size",
        )?;
        non_negative(self.send_rate, "send_rate")?;
        non_negative(self.recv_rate, "recv_rate")?;
        Ok(())
    }
}

fn default_laddr() -> String {
    "tcp://0.0.0.0:26656".to_owned()
}

fn default_addr_book_file() -> String {
    format!("{CONFIG_DIR}/addrbook.json")
}

fn default_max_num_inbound_peers() -> i64 {
    40
}

fn default_max_num_outbound_peers() -> i64 {
    10
}

fn default_flush_throttle_timeout() -> Duration {
    Duration::from_millis(100)
}

fn default_max_packet_msg_payload_size() -> i64 {
    1024
}

fn default_send_rate() -> i64 {
    5_120_000
}

fn default_recv_rate() -> i64 {
    5_120_000
}

fn default_handshake_timeout() -> Duration {
    Duration::from_secs(20)
}

fn default_dial_timeout() -> Duration {
    Duration::from_secs(3)
}
