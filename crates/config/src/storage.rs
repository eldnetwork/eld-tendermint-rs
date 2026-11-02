//! `config.StorageConfig`.

use serde::Deserialize;

/// Storage options. Go does not run `ValidateBasic` on this section.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub discard_abci_responses: bool,
}

impl StorageConfig {
    /// How often the catch-up thread retries pruning blocks the current summary already covers.
    pub const CATCH_UP_PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5 * 60);

    #[must_use]
    pub fn default_config() -> Self {
        Self {
            discard_abci_responses: false,
        }
    }

    /// `TestStorageConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self::default_config()
    }
}
