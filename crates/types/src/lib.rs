//! Tendermint 0.34 core types: headers, votes, validators, genesis, and blocks.
//!
//! Matches `types` in the Go tree for data, `ValidateBasic`, header and merkle hashes,
//! length-prefixed canonical sign bytes, and proposer-priority rotation. Executing
//! transactions is later.

mod bits;
mod block;
mod block_id;
mod cdc;
mod commit;
mod error;
mod evidence;
mod genesis;
mod hash;
mod header;
mod params;
mod part;
mod proposal;
mod time;
mod tx;
mod validator;
mod vote;

pub use bits::BitArray;
pub use block::Block;
pub use block_id::{BlockId, PartSetHeader};
pub use commit::{Commit, CommitSig};
pub use error::Error;
pub use evidence::{DuplicateVoteEvidence, EvidenceList};
pub use genesis::{GenesisDoc, GenesisValidator};
pub use hash::{ChainId, Hash, validate_hash};
pub use header::{ConsensusVersion, Header};
pub use params::{
    ABCI_PUBKEY_TYPE_ED25519, ABCI_PUBKEY_TYPE_SECP256K1, BlockParams, ConsensusParams, Duration,
    EvidenceParams, ValidatorParams, VersionParams, hash_consensus_params,
};
pub use part::{Part, PartSet};
pub use proposal::Proposal;
pub use time::Time;
pub use tx::{Tx, Txs};
pub use validator::{Validator, ValidatorSet};
pub use vote::Vote;

pub use eld_tendermint_crypto::ADDRESS_SIZE;
pub use eld_tendermint_crypto::Address;
pub use eld_tendermint_proto::types::{BlockIdFlag, SignedMsgType};

/// `types.MaxChainIDLen`.
pub const MAX_CHAIN_ID_LEN: usize = 50;

/// `tmhash.Size`.
pub const HASH_SIZE: usize = eld_tendermint_crypto::TMHASH_SIZE;

/// `types.MaxSignatureSize` for ed25519 (64).
pub const MAX_SIGNATURE_SIZE: usize = 64;

/// `version.BlockProtocol` for Tendermint 0.34.
pub const BLOCK_PROTOCOL: u64 = 11;

/// `types.MaxTotalVotingPower` (`math.MaxInt64 / 8`).
pub const MAX_TOTAL_VOTING_POWER: i64 = i64::MAX / 8;

/// `types.MaxBlockSizeBytes` (100 MiB).
pub const MAX_BLOCK_SIZE_BYTES: i64 = 104_857_600;

/// `types.BlockPartSizeBytes` (64 KiB).
pub const BLOCK_PART_SIZE_BYTES: u32 = 65_536;

/// `types.MaxBlockPartsCount` (`MaxBlockSizeBytes / BlockPartSizeBytes + 1`).
pub const MAX_BLOCK_PARTS_COUNT: u32 = (MAX_BLOCK_SIZE_BYTES as u32 / BLOCK_PART_SIZE_BYTES) + 1;
