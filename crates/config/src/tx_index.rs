//! `config.TxIndexConfig`.

use serde::Deserialize;

/// Transaction indexer options. Go does not run `ValidateBasic` on this section.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct TxIndexConfig {
    #[serde(default = "default_indexer")]
    pub indexer: String,
    #[serde(rename = "psql-conn", default)]
    pub psql_conn: String,
}

impl TxIndexConfig {
    #[must_use]
    pub fn default_config() -> Self {
        Self {
            indexer: default_indexer(),
            psql_conn: String::new(),
        }
    }

    /// `TestTxIndexConfig`.
    #[must_use]
    pub fn test_config() -> Self {
        Self::default_config()
    }
}

fn default_indexer() -> String {
    "kv".to_owned()
}
