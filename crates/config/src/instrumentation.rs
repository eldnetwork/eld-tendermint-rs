//! `config.InstrumentationConfig`.

use serde::Deserialize;

use crate::error::{Error, non_negative};

/// Prometheus options.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct InstrumentationConfig {
    #[serde(default)]
    pub prometheus: bool,
    #[serde(default = "default_prometheus_listen_addr")]
    pub prometheus_listen_addr: String,
    #[serde(default = "default_max_open_connections")]
    pub max_open_connections: i64,
    #[serde(default = "default_namespace")]
    pub namespace: String,
}

impl InstrumentationConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            prometheus: false,
            prometheus_listen_addr: default_prometheus_listen_addr(),
            max_open_connections: default_max_open_connections(),
            namespace: default_namespace(),
        }
    }

    /// `TestInstrumentationConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self::default_config()
    }

    /// `InstrumentationConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when `max_open_connections` is negative.
    pub fn validate_basic(&self) -> Result<(), Error> {
        non_negative(self.max_open_connections, "max_open_connections")
    }
}

fn default_prometheus_listen_addr() -> String {
    ":26660".to_owned()
}

fn default_max_open_connections() -> i64 {
    3
}

fn default_namespace() -> String {
    "tendermint".to_owned()
}
