//! Consensus parameters needed by genesis and `Header.ConsensusHash`.
//!
//! `UpdateConsensusParams` is left for block execution.

use prost::Message;

use eld_tendermint_crypto::sum;

use crate::{Error, Hash, MAX_BLOCK_SIZE_BYTES};

/// ABCI name for ed25519 (`ed25519.KeyType`), not the Amino type string.
pub const ABCI_PUBKEY_TYPE_ED25519: &str = "ed25519";

/// ABCI name for secp256k1. Accepted in params even though this crate only loads ed25519 keys.
pub const ABCI_PUBKEY_TYPE_SECP256K1: &str = "secp256k1";

/// `time.Duration` as seconds and nanoseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Duration {
    pub seconds: i64,
    pub nanos: i32,
}

impl Duration {
    #[must_use]
    pub const fn from_hours(hours: i64) -> Self {
        Self {
            seconds: hours * 3_600,
            nanos: 0,
        }
    }

    /// Splits a nanosecond count into seconds and nanoseconds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInteger`] when the second count does not fit in `i64`.
    pub fn from_nanos(total: i128) -> Result<Self, Error> {
        let seconds = total.div_euclid(1_000_000_000);
        let nanos = total.rem_euclid(1_000_000_000);
        let seconds = i64::try_from(seconds).map_err(|_| Error::InvalidInteger)?;
        let nanos = i32::try_from(nanos).map_err(|_| Error::InvalidInteger)?;
        Ok(Self { seconds, nanos })
    }

    #[must_use]
    pub fn total_nanos(self) -> i128 {
        i128::from(self.seconds) * 1_000_000_000 + i128::from(self.nanos)
    }

    #[must_use]
    pub fn is_positive(self) -> bool {
        self.seconds > 0 || (self.seconds == 0 && self.nanos > 0)
    }
}

/// `types.BlockParams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockParams {
    pub max_bytes: i64,
    pub max_gas: i64,
    pub time_iota_ms: i64,
}

/// `types.EvidenceParams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvidenceParams {
    pub max_age_num_blocks: i64,
    pub max_age_duration: Duration,
    pub max_bytes: i64,
}

/// `types.ValidatorParams`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorParams {
    pub pub_key_types: Vec<String>,
}

/// `types.VersionParams`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VersionParams {
    pub app_version: u64,
}

/// `types.ConsensusParams`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsensusParams {
    pub block: BlockParams,
    pub evidence: EvidenceParams,
    pub validator: ValidatorParams,
    pub version: VersionParams,
}

impl ConsensusParams {
    /// `DefaultConsensusParams`.
    #[must_use]
    pub fn default_params() -> Self {
        Self {
            block: BlockParams {
                max_bytes: 22_020_096,
                max_gas: -1,
                time_iota_ms: 1_000,
            },
            evidence: EvidenceParams {
                max_age_num_blocks: 100_000,
                max_age_duration: Duration::from_hours(48),
                max_bytes: 1_048_576,
            },
            validator: ValidatorParams {
                pub_key_types: vec![ABCI_PUBKEY_TYPE_ED25519.to_owned()],
            },
            version: VersionParams { app_version: 0 },
        }
    }

    /// `ValidateConsensusParams`.
    ///
    /// # Errors
    ///
    /// Returns an error when a field is outside the Go limits.
    pub fn validate(&self) -> Result<(), Error> {
        if self.block.max_bytes <= 0 {
            return Err(Error::BlockMaxBytesNotPositive {
                got: self.block.max_bytes,
            });
        }
        if self.block.max_bytes > MAX_BLOCK_SIZE_BYTES {
            return Err(Error::BlockMaxBytesTooBig {
                got: self.block.max_bytes,
            });
        }
        if self.block.max_gas < -1 {
            return Err(Error::BlockMaxGasTooSmall {
                got: self.block.max_gas,
            });
        }
        if self.block.time_iota_ms <= 0 {
            return Err(Error::TimeIotaNotPositive {
                got: self.block.time_iota_ms,
            });
        }
        if self.evidence.max_age_num_blocks <= 0 {
            return Err(Error::EvidenceMaxAgeBlocksNotPositive {
                got: self.evidence.max_age_num_blocks,
            });
        }
        if !self.evidence.max_age_duration.is_positive() {
            return Err(Error::EvidenceMaxAgeDurationNotPositive);
        }
        if self.evidence.max_bytes > self.block.max_bytes {
            return Err(Error::EvidenceMaxBytesTooBig {
                got: self.evidence.max_bytes,
            });
        }
        if self.evidence.max_bytes < 0 {
            return Err(Error::EvidenceMaxBytesNegative {
                got: self.evidence.max_bytes,
            });
        }
        if self.validator.pub_key_types.is_empty() {
            return Err(Error::NoPubKeyTypes);
        }
        for key_type in &self.validator.pub_key_types {
            if key_type != ABCI_PUBKEY_TYPE_ED25519 && key_type != ABCI_PUBKEY_TYPE_SECP256K1 {
                return Err(Error::UnknownPubKeyType {
                    got: key_type.clone(),
                });
            }
        }
        Ok(())
    }
}

impl Default for ConsensusParams {
    fn default() -> Self {
        Self::default_params()
    }
}

/// `HashConsensusParams`: tmhash of `HashedParams{block_max_bytes, block_max_gas}`.
#[must_use]
pub fn hash_consensus_params(params: &ConsensusParams) -> Hash {
    let hashed = eld_tendermint_proto::types::HashedParams {
        block_max_bytes: params.block.max_bytes,
        block_max_gas: params.block.max_gas,
    };
    Hash::from_array(sum(&hashed.encode_to_vec()))
}
