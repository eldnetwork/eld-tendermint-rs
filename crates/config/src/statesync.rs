//! `config.StateSyncConfig`.

use serde::Deserialize;
use serde::de::{self, Deserializer, SeqAccess, Visitor};

use crate::Error;
use crate::duration::Duration;

const FIVE_SECONDS: i64 = 5_000_000_000;

/// State sync options. Checks apply only when `enable` is true.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StateSyncConfig {
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub temp_dir: String,
    /// A TOML array, or the comma-separated string Go writes into `config.toml`.
    #[serde(default, deserialize_with = "deserialize_rpc_servers")]
    pub rpc_servers: Vec<String>,
    #[serde(default = "default_trust_period")]
    pub trust_period: Duration,
    #[serde(default)]
    pub trust_height: i64,
    #[serde(default)]
    pub trust_hash: String,
    #[serde(default = "default_discovery_time")]
    pub discovery_time: Duration,
    #[serde(default = "default_chunk_request_timeout")]
    pub chunk_request_timeout: Duration,
    #[serde(default = "default_chunk_fetchers")]
    pub chunk_fetchers: i64,
}

impl StateSyncConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            enable: false,
            temp_dir: String::new(),
            rpc_servers: Vec::new(),
            trust_period: default_trust_period(),
            trust_height: 0,
            trust_hash: String::new(),
            discovery_time: default_discovery_time(),
            chunk_request_timeout: default_chunk_request_timeout(),
            chunk_fetchers: default_chunk_fetchers(),
        }
    }

    /// `TestStateSyncConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self::default_config()
    }

    /// Decodes `trust_hash` as hex.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidTrustHash`] when the string is not hex.
    pub fn trust_hash_bytes(&self) -> Result<Vec<u8>, Error> {
        hex::decode(&self.trust_hash).map_err(|_| Error::InvalidTrustHash)
    }

    /// `StateSyncConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns an error when state sync is enabled and a required field is missing or out of range.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if !self.enable {
            return Ok(());
        }
        if self.rpc_servers.is_empty() {
            return Err(Error::RpcServersRequired);
        }
        if self.rpc_servers.len() < 2 {
            return Err(Error::RpcServersTooFew);
        }
        if self.rpc_servers.iter().any(String::is_empty) {
            return Err(Error::EmptyRpcServer);
        }
        let discovery = self.discovery_time.as_nanos();
        if discovery != 0 && discovery < FIVE_SECONDS {
            return Err(Error::DiscoveryTimeTooSmall);
        }
        if self.trust_period.as_nanos() <= 0 {
            return Err(Error::TrustPeriodRequired);
        }
        if self.trust_height <= 0 {
            return Err(Error::TrustHeightRequired);
        }
        if self.trust_hash.is_empty() {
            return Err(Error::TrustHashRequired);
        }
        hex::decode(&self.trust_hash).map_err(|_| Error::InvalidTrustHash)?;
        if self.chunk_request_timeout.as_nanos() < FIVE_SECONDS {
            return Err(Error::ChunkRequestTimeoutTooSmall);
        }
        if self.chunk_fetchers <= 0 {
            return Err(Error::ChunkFetchersRequired);
        }
        Ok(())
    }
}

fn default_trust_period() -> Duration {
    Duration::from_secs(168 * 60 * 60)
}

fn default_discovery_time() -> Duration {
    Duration::from_secs(15)
}

fn default_chunk_request_timeout() -> Duration {
    Duration::from_secs(10)
}

fn default_chunk_fetchers() -> i64 {
    4
}

/// Go's template joins this list with commas, so an empty list is `""`.
fn deserialize_rpc_servers<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct RpcServersVisitor;

    impl<'de> Visitor<'de> for RpcServersVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a comma-separated string or an array of strings for rpc_servers")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(value
                .split(',')
                .map(str::trim)
                .filter(|server| !server.is_empty())
                .map(ToOwned::to_owned)
                .collect())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut servers = Vec::with_capacity(seq.size_hint().unwrap_or(0));
            while let Some(server) = seq.next_element::<String>()? {
                servers.push(server);
            }
            Ok(servers)
        }
    }

    deserializer.deserialize_any(RpcServersVisitor)
}
