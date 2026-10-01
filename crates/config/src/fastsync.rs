//! `config.FastSyncConfig`.

use serde::Deserialize;

use crate::Error;

/// Fast sync options.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct FastSyncConfig {
    #[serde(default = "default_version")]
    pub version: String,
}

impl FastSyncConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            version: default_version(),
        }
    }

    /// `TestFastSyncConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self::default_config()
    }

    /// `FastSyncConfig.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownFastSyncVersion`] when `version` is not `v0`, `v1`, or `v2`.
    pub fn validate_basic(&self) -> Result<(), Error> {
        match self.version.as_str() {
            "v0" | "v1" | "v2" => Ok(()),
            _ => Err(Error::UnknownFastSyncVersion {
                got: self.version.clone(),
            }),
        }
    }
}

fn default_version() -> String {
    "v0".to_owned()
}
