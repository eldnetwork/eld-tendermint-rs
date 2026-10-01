//! `config.StorageConfig`.

use serde::Deserialize;

/// Storage options. Go does not run `ValidateBasic` on this section.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct StorageConfig {
    #[serde(default)]
    pub discard_abci_responses: bool,
}

impl StorageConfig {
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
