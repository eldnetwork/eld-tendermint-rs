//! `config.StorageConfig`.

use serde::Deserialize;

/// Storage options. Go does not run `ValidateBasic` on this section.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub discard_abci_responses: bool,
}

impl StorageConfig {
    /// How often the node retries pruning blocks the current summary already covers.
    pub const PRUNE_INTERVAL: crate::Duration = crate::Duration::from_secs(5 * 60);

    /// Largest total size of rotated WAL segments that replay still needs.
    pub const WAL_SIZE_LIMIT: u64 = 1024 * 1024 * 1024;

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
