//! `config.RPCConfig`.

use std::path::PathBuf;

use serde::Deserialize;

use crate::duration::Duration;
use crate::error::{Error, non_negative};
use crate::path::{go_join, is_abs, rootify};
use crate::{CONFIG_DIR, DEFAULT_SUBSCRIPTION_BUFFER_SIZE, MIN_SUBSCRIPTION_BUFFER_SIZE};

/// RPC server options.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RpcConfig {
    #[serde(rename = "home", default)]
    pub root_dir: String,
    #[serde(default = "default_laddr")]
    pub laddr: String,
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    #[serde(default = "default_cors_methods")]
    pub cors_allowed_methods: Vec<String>,
    #[serde(default = "default_cors_headers")]
    pub cors_allowed_headers: Vec<String>,
    #[serde(default)]
    pub grpc_laddr: String,
    #[serde(default = "default_grpc_max_open_connections")]
    pub grpc_max_open_connections: i64,
    #[serde(rename = "unsafe", default)]
    pub unsafe_commands: bool,
    #[serde(default = "default_max_open_connections")]
    pub max_open_connections: i64,
    #[serde(default = "default_max_subscription_clients")]
    pub max_subscription_clients: i64,
    #[serde(default = "default_max_subscriptions_per_client")]
    pub max_subscriptions_per_client: i64,
    #[serde(default = "default_subscription_buffer_size")]
    pub experimental_subscription_buffer_size: i64,
    #[serde(default = "default_websocket_write_buffer_size")]
    pub experimental_websocket_write_buffer_size: i64,
    #[serde(default)]
    pub experimental_close_on_slow_client: bool,
    #[serde(default = "default_timeout_broadcast_tx_commit")]
    pub timeout_broadcast_tx_commit: Duration,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: i64,
    #[serde(default = "default_max_header_bytes")]
    pub max_header_bytes: i64,
    #[serde(default)]
    pub tls_cert_file: String,
    #[serde(default)]
    pub tls_key_file: String,
    #[serde(default)]
    pub pprof_laddr: String,
}

impl RpcConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            root_dir: String::new(),
            laddr: default_laddr(),
            cors_allowed_origins: Vec::new(),
            cors_allowed_methods: default_cors_methods(),
            cors_allowed_headers: default_cors_headers(),
            grpc_laddr: String::new(),
            grpc_max_open_connections: default_grpc_max_open_connections(),
            unsafe_commands: false,
            max_open_connections: default_max_open_connections(),
            max_subscription_clients: default_max_subscription_clients(),
            max_subscriptions_per_client: default_max_subscriptions_per_client(),
            experimental_subscription_buffer_size: default_subscription_buffer_size(),
            experimental_websocket_write_buffer_size: default_websocket_write_buffer_size(),
            experimental_close_on_slow_client: false,
            timeout_broadcast_tx_commit: default_timeout_broadcast_tx_commit(),
            max_body_bytes: default_max_body_bytes(),
            max_header_bytes: default_max_header_bytes(),
            tls_cert_file: String::new(),
            tls_key_file: String::new(),
            pprof_laddr: String::new(),
        }
    }

    /// `TestRPCConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        let mut cfg = Self::default_config();
        cfg.laddr = "tcp://127.0.0.1:36657".to_owned();
        cfg.grpc_laddr = "tcp://127.0.0.1:36658".to_owned();
        cfg.unsafe_commands = true;
        cfg
    }

    #[must_use]
    pub fn is_cors_enabled(&self) -> bool {
        !self.cors_allowed_origins.is_empty()
    }

    #[must_use]
    pub fn key_file(&self) -> PathBuf {
        tls_path(&self.tls_key_file, &self.root_dir)
    }

    #[must_use]
    pub fn cert_file(&self) -> PathBuf {
        tls_path(&self.tls_cert_file, &self.root_dir)
    }

    #[must_use]
    pub fn is_tls_enabled(&self) -> bool {
        !self.tls_cert_file.is_empty() && !self.tls_key_file.is_empty()
    }

    /// `RPCConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when a limit is negative or a subscription buffer is too small.
    pub fn validate_basic(&self) -> Result<(), Error> {
        non_negative(self.grpc_max_open_connections, "grpc_max_open_connections")?;
        non_negative(self.max_open_connections, "max_open_connections")?;
        non_negative(self.max_subscription_clients, "max_subscription_clients")?;
        non_negative(
            self.max_subscriptions_per_client,
            "max_subscriptions_per_client",
        )?;
        if self.experimental_subscription_buffer_size < MIN_SUBSCRIPTION_BUFFER_SIZE {
            return Err(Error::SubscriptionBufferTooSmall {
                got: self.experimental_subscription_buffer_size,
            });
        }
        if self.experimental_websocket_write_buffer_size
            < self.experimental_subscription_buffer_size
        {
            return Err(Error::WebSocketBufferTooSmall {
                subscription: self.experimental_subscription_buffer_size,
            });
        }
        non_negative(
            self.timeout_broadcast_tx_commit.as_nanos(),
            "timeout_broadcast_tx_commit",
        )?;
        non_negative(self.max_body_bytes, "max_body_bytes")?;
        non_negative(self.max_header_bytes, "max_header_bytes")?;
        Ok(())
    }
}

fn tls_path(path: &str, root: &str) -> PathBuf {
    if is_abs(path) {
        PathBuf::from(path)
    } else {
        rootify(&go_join(CONFIG_DIR, path), root)
    }
}

fn default_laddr() -> String {
    "tcp://127.0.0.1:26657".to_owned()
}

fn default_cors_methods() -> Vec<String> {
    ["HEAD", "GET", "POST"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

fn default_cors_headers() -> Vec<String> {
    [
        "Origin",
        "Accept",
        "Content-Type",
        "X-Requested-With",
        "X-Server-Time",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn default_grpc_max_open_connections() -> i64 {
    900
}

fn default_max_open_connections() -> i64 {
    900
}

fn default_max_subscription_clients() -> i64 {
    100
}

fn default_max_subscriptions_per_client() -> i64 {
    5
}

fn default_subscription_buffer_size() -> i64 {
    DEFAULT_SUBSCRIPTION_BUFFER_SIZE
}

fn default_websocket_write_buffer_size() -> i64 {
    DEFAULT_SUBSCRIPTION_BUFFER_SIZE
}

fn default_timeout_broadcast_tx_commit() -> Duration {
    Duration::from_secs(10)
}

fn default_max_body_bytes() -> i64 {
    1_000_000
}

fn default_max_header_bytes() -> i64 {
    1 << 20
}
