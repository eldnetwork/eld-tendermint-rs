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
mod light;
mod log;
mod params;
mod part;
mod proposal;
mod time;
mod tx;
mod validator;
mod vote;
mod vote_set;

pub use bits::BitArray;
pub use block::{Block, max_commit_bytes, max_data_bytes};
pub use block_id::{BlockId, PartSetHeader};
pub use commit::{Commit, CommitSig, median_time};
pub use error::Error;
pub use evidence::{DuplicateVoteEvidence, Evidence, EvidenceList, LightClientAttackEvidence};
pub use genesis::{GenesisDoc, GenesisValidator};
pub use hash::{ChainId, Hash, validate_hash};
pub use header::{ConsensusVersion, Header};
pub use light::{LightBlock, SignedHeader};
pub use log::{Level, log_line, set_log_capture, upper_hex};
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
pub use vote_set::VoteSet;

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

/// `types.MaxHeaderBytes`. A soft cap; the app hash is not bounded.
pub const MAX_HEADER_BYTES: i64 = 626;

/// `types.MaxOverheadForBlock`. Protobuf overhead for a block, excluding txs.
pub const MAX_OVERHEAD_FOR_BLOCK: i64 = 11;

/// `types.MaxCommitOverheadBytes`. Commit without any signatures.
pub const MAX_COMMIT_OVERHEAD_BYTES: i64 = 94;

/// `types.MaxCommitSigBytes`. One signature, address, flag, and timestamp.
pub const MAX_COMMIT_SIG_BYTES: i64 = 109;

/// `types.MaxBlockSizeBytes` (100 MiB).
pub const MAX_BLOCK_SIZE_BYTES: i64 = 104_857_600;

/// `types.BlockPartSizeBytes` (64 KiB).
pub const BLOCK_PART_SIZE_BYTES: u32 = 65_536;

/// `types.MaxBlockPartsCount` (`MaxBlockSizeBytes / BlockPartSizeBytes + 1`).
pub const MAX_BLOCK_PARTS_COUNT: u32 = (MAX_BLOCK_SIZE_BYTES as u32 / BLOCK_PART_SIZE_BYTES) + 1;
