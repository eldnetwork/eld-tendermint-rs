use std::fmt;

use eld_tendermint_crypto::Error as CryptoError;

/// Failures from validation, JSON, and proto conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NegativeHeight,
    ZeroHeight,
    NegativeRound,
    NegativePolRound,
    ChainIdEmpty,
    ChainIdTooLong {
        len: usize,
    },
    InvalidHashLength {
        len: usize,
    },
    InvalidAddressLength {
        len: usize,
    },
    WrongBlockProtocol {
        got: u64,
    },
    BlockIdMustBeComplete,
    CommitForNilBlock,
    InvalidVoteType,
    InvalidProposalType,
    MissingSignature,
    SignatureTooBig {
        len: usize,
    },
    NegativeValidatorIndex,
    UnknownBlockIdFlag,
    AbsentHasAddress,
    AbsentHasTime,
    AbsentHasSignature,
    NoCommitSignatures,
    MissingPubKey,
    NegativeVotingPower,
    ZeroVotingPower,
    EmptyValidatorSet,
    NonPositiveTimes,
    DuplicateValidator,
    MissingProposer,
    VotingPowerTooHigh,
    ValidatorVotingPowerTooHigh {
        got: i64,
    },
    AddressMismatch,
    BlockMaxBytesNotPositive {
        got: i64,
    },
    BlockMaxBytesTooBig {
        got: i64,
    },
    BlockMaxGasTooSmall {
        got: i64,
    },
    TimeIotaNotPositive {
        got: i64,
    },
    EvidenceMaxAgeBlocksNotPositive {
        got: i64,
    },
    EvidenceMaxAgeDurationNotPositive,
    EvidenceMaxBytesTooBig {
        got: i64,
    },
    EvidenceMaxBytesNegative {
        got: i64,
    },
    NoPubKeyTypes,
    UnknownPubKeyType {
        got: String,
    },
    InvalidTime,
    InvalidHex,
    InvalidInteger,
    InvalidBitArray {
        detail: String,
    },
    Json(String),
    PubKey(CryptoError),
    Signature(CryptoError),
    PartTooBig {
        len: usize,
    },
    ZeroPartSize,
    TooManyParts {
        count: usize,
    },
    UnexpectedPartIndex {
        index: u32,
        total: u32,
    },
    InvalidPartProof,
    IncompletePartSet,
    MissingPart,
    Proof(CryptoError),
    MissingHeader,
    NilLastCommit,
    WrongLastCommitHash,
    WrongDataHash,
    WrongEvidenceHash,
    /// Light-client attack evidence is not decoded.
    UnsupportedEvidence,
    DuplicateVoteOrder,
    MissingEvidenceVote,
    ValidatorNotInSet,
    EvidenceHeightRoundTypeMismatch,
    EvidenceAddressMismatch,
    EvidenceSameBlockId,
    EvidencePubKeyMismatch,
    EvidenceValidatorPowerMismatch,
    EvidenceTotalPowerMismatch,
    UnexpectedVoteStep,
    InvalidVoteIndex,
    ConflictingVote,
    DuplicateVoteSignature,
    MissingConflictingBlock,
    MissingSignedHeader,
    MissingLightValidatorSet,
    MissingTrustedHeader,
    ValidatorHashMismatch,
    HeaderCommitMismatch,
    CommitSignsWrongBlock,
    ChainIdMismatch,
    NonPositiveTotalVotingPower,
    NonPositiveCommonHeight,
    CommonHeightAhead,
    CommitSignatureCount {
        expected: usize,
        got: usize,
    },
    CommitHeightMismatch,
    CommitBlockIdMismatch,
    NotEnoughVotingPower {
        got: i64,
        needed: i64,
    },
    DoubleCommitVote,
    ConflictingHeaderDerived,
    TrustedHashMatches,
    ConflictingTimeOrder,
    ByzantineValidatorMismatch,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeHeight => write!(f, "negative height"),
            Self::ZeroHeight => write!(f, "zero height"),
            Self::NegativeRound => write!(f, "negative round"),
            Self::NegativePolRound => write!(f, "negative POL round"),
            Self::ChainIdEmpty => write!(f, "chain id is empty"),
            Self::ChainIdTooLong { len } => {
                write!(
                    f,
                    "chain id is too long: got {len}, max {MAX}",
                    MAX = crate::MAX_CHAIN_ID_LEN
                )
            }
            Self::InvalidHashLength { len } => {
                write!(
                    f,
                    "invalid hash length: got {len}, expected 0 or {}",
                    crate::HASH_SIZE
                )
            }
            Self::InvalidAddressLength { len } => {
                write!(
                    f,
                    "invalid address length: got {len}, expected {}",
                    crate::ADDRESS_SIZE
                )
            }
            Self::WrongBlockProtocol { got } => {
                write!(
                    f,
                    "block protocol is {got}, expected {}",
                    crate::BLOCK_PROTOCOL
                )
            }
            Self::BlockIdMustBeComplete => write!(f, "block id must be complete"),
            Self::CommitForNilBlock => write!(f, "commit cannot be for a nil block"),
            Self::InvalidVoteType => write!(f, "invalid vote type"),
            Self::InvalidProposalType => write!(f, "invalid proposal type"),
            Self::MissingSignature => write!(f, "signature is missing"),
            Self::SignatureTooBig { len } => {
                write!(
                    f,
                    "signature is too big: got {len}, max {}",
                    crate::MAX_SIGNATURE_SIZE
                )
            }
            Self::NegativeValidatorIndex => write!(f, "negative validator index"),
            Self::UnknownBlockIdFlag => write!(f, "unknown block id flag"),
            Self::AbsentHasAddress => write!(f, "absent commit has a validator address"),
            Self::AbsentHasTime => write!(f, "absent commit has a timestamp"),
            Self::AbsentHasSignature => write!(f, "absent commit has a signature"),
            Self::NoCommitSignatures => write!(f, "no signatures in commit"),
            Self::MissingPubKey => write!(f, "validator does not have a public key"),
            Self::NegativeVotingPower => write!(f, "validator has negative voting power"),
            Self::ZeroVotingPower => write!(f, "validator has no voting power"),
            Self::EmptyValidatorSet => write!(f, "validator set is empty"),
            Self::NonPositiveTimes => write!(
                f,
                "Cannot call IncrementProposerPriority with non-positive times"
            ),
            Self::DuplicateValidator => write!(f, "duplicate validator address"),
            Self::MissingProposer => write!(f, "validator set has no proposer"),
            Self::VotingPowerTooHigh => write!(f, "total voting power is too high"),
            Self::ValidatorVotingPowerTooHigh { got } => write!(
                f,
                "to prevent clipping/overflow, voting power can't be higher than {}, got {got}",
                crate::MAX_TOTAL_VOTING_POWER
            ),
            Self::AddressMismatch => write!(f, "validator address does not match public key"),
            Self::BlockMaxBytesNotPositive { got } => {
                write!(f, "block max bytes must be greater than 0, got {got}")
            }
            Self::BlockMaxBytesTooBig { got } => write!(f, "block max bytes is too big: {got}"),
            Self::BlockMaxGasTooSmall { got } => {
                write!(f, "block max gas must be at least -1, got {got}")
            }
            Self::TimeIotaNotPositive { got } => {
                write!(f, "time iota must be greater than 0, got {got}")
            }
            Self::EvidenceMaxAgeBlocksNotPositive { got } => {
                write!(
                    f,
                    "evidence max age in blocks must be greater than 0, got {got}"
                )
            }
            Self::EvidenceMaxAgeDurationNotPositive => {
                write!(f, "evidence max age duration must be greater than 0")
            }
            Self::EvidenceMaxBytesTooBig { got } => {
                write!(f, "evidence max bytes exceeds block max bytes: {got}")
            }
            Self::EvidenceMaxBytesNegative { got } => {
                write!(f, "evidence max bytes must be non-negative, got {got}")
            }
            Self::NoPubKeyTypes => write!(f, "validator pubkey types must be non-empty"),
            Self::UnknownPubKeyType { got } => write!(f, "unknown validator pubkey type {got:?}"),
            Self::InvalidTime => write!(f, "invalid time"),
            Self::InvalidHex => write!(f, "invalid hex"),
            Self::InvalidInteger => write!(f, "invalid integer"),
            Self::InvalidBitArray { detail } => write!(f, "invalid bit array: {detail}"),
            Self::Json(msg) => write!(f, "invalid genesis json: {msg}"),
            Self::PubKey(err) => write!(f, "invalid public key: {err}"),
            Self::Signature(err) => write!(f, "invalid signature: {err}"),
            Self::PartTooBig { len } => {
                write!(
                    f,
                    "too big: {len} bytes, max: {}",
                    crate::BLOCK_PART_SIZE_BYTES
                )
            }
            Self::ZeroPartSize => write!(f, "part size must be greater than 0"),
            Self::TooManyParts { count } => write!(f, "too many parts: {count}"),
            Self::UnexpectedPartIndex { index, total } => {
                write!(f, "unexpected part index {index}, total {total}")
            }
            Self::InvalidPartProof => write!(f, "invalid part proof"),
            Self::IncompletePartSet => write!(f, "incomplete part set"),
            Self::MissingPart => write!(f, "nil part"),
            Self::Proof(err) => write!(f, "invalid proof: {err}"),
            Self::MissingHeader => write!(f, "nil Header"),
            Self::NilLastCommit => write!(f, "nil LastCommit"),
            Self::WrongLastCommitHash => write!(f, "wrong Header.LastCommitHash"),
            Self::WrongDataHash => write!(f, "wrong Header.DataHash"),
            Self::WrongEvidenceHash => write!(f, "wrong Header.EvidenceHash"),
            Self::UnsupportedEvidence => write!(f, "evidence is not supported yet"),
            Self::DuplicateVoteOrder => write!(f, "duplicate votes in invalid order"),
            Self::MissingEvidenceVote => write!(f, "one or both of the votes are empty"),
            Self::ValidatorNotInSet => write!(f, "address was not a validator"),
            Self::EvidenceHeightRoundTypeMismatch => write!(f, "h/r/s does not match"),
            Self::EvidenceAddressMismatch => write!(f, "validator addresses do not match"),
            Self::EvidenceSameBlockId => {
                write!(f, "block IDs are the same - not a real duplicate vote")
            }
            Self::EvidencePubKeyMismatch => write!(f, "address doesn't match pubkey"),
            Self::EvidenceValidatorPowerMismatch => {
                write!(
                    f,
                    "validator power from evidence and our validator set does not match"
                )
            }
            Self::EvidenceTotalPowerMismatch => {
                write!(
                    f,
                    "total voting power from the evidence and our validator set does not match"
                )
            }
            Self::UnexpectedVoteStep => write!(f, "unexpected vote step"),
            Self::InvalidVoteIndex => write!(f, "invalid vote validator index"),
            Self::ConflictingVote => write!(f, "conflicting vote"),
            Self::DuplicateVoteSignature => write!(f, "vote signature is non-deterministic"),
            Self::MissingConflictingBlock => write!(f, "conflicting block is nil"),
            Self::MissingSignedHeader => write!(f, "missing signed header"),
            Self::MissingLightValidatorSet => write!(f, "missing validator set"),
            Self::MissingTrustedHeader => write!(f, "missing trusted header"),
            Self::ValidatorHashMismatch => {
                write!(f, "validator hash does not match the validator set")
            }
            Self::HeaderCommitMismatch => write!(f, "header and commit height mismatch"),
            Self::CommitSignsWrongBlock => write!(f, "commit signs a different block"),
            Self::ChainIdMismatch => write!(f, "header belongs to another chain"),
            Self::NonPositiveTotalVotingPower => write!(f, "negative or zero total voting power"),
            Self::NonPositiveCommonHeight => write!(f, "negative or zero common height"),
            Self::CommonHeightAhead => {
                write!(f, "common height is ahead of the conflicting block height")
            }
            Self::CommitSignatureCount { expected, got } => {
                write!(
                    f,
                    "invalid commit -- wrong signature count: want {expected}, got {got}"
                )
            }
            Self::CommitHeightMismatch => write!(f, "invalid commit -- wrong height"),
            Self::CommitBlockIdMismatch => write!(f, "invalid commit -- wrong block ID"),
            Self::NotEnoughVotingPower { got, needed } => {
                write!(f, "not enough voting power: got {got}, needed {needed}")
            }
            Self::DoubleCommitVote => write!(f, "double vote in commit"),
            Self::ConflictingHeaderDerived => write!(
                f,
                "common height matches the conflicting block but the header was not derived"
            ),
            Self::TrustedHashMatches => {
                write!(f, "trusted header hash matches the conflicting header")
            }
            Self::ConflictingTimeOrder => write!(
                f,
                "conflicting block does not violate monotonically increasing time"
            ),
            Self::ByzantineValidatorMismatch => {
                write!(f, "byzantine validators do not match the commits")
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PubKey(err) | Self::Proof(err) | Self::Signature(err) => Some(err),
            _ => None,
        }
    }
}
