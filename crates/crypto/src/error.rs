use std::fmt;

/// Failures from key construction, Amino JSON, Merkle proofs, and proto conversion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Byte length is not the size Go requires for this key.
    InvalidLength { expected: usize, got: usize },
    /// Private-key suffix is 32 zero bytes, so Go's `PubKey` would panic.
    UninitializedPrivKey,
    /// Envelope is not a `type`/`value` JSON object of strings.
    InvalidAminoJson,
    /// `type` is present and is not the name this key uses.
    WrongAminoType { expected: &'static str, got: String },
    /// `value` is not standard base64 with padding.
    InvalidBase64,
    /// Proto `PublicKey` is missing, or its sum is not Ed25519.
    UnsupportedKeyType,
    /// `Proof.Total` is negative.
    NegativeProofTotal,
    /// `Proof.Index` is negative.
    NegativeProofIndex,
    /// `Proof.LeafHash` is not `tmhash.Size` bytes.
    InvalidLeafHash { got: usize },
    /// `Proof.Aunts` is longer than [`crate::MAX_AUNTS`].
    TooManyAunts { got: usize },
    /// One aunt hash is not `tmhash.Size` bytes.
    InvalidAuntHash { index: usize, got: usize },
    /// Recomputed leaf hash does not match `Proof.LeafHash`.
    ProofLeafMismatch,
    /// Recomputed root does not match the expected root.
    ProofRootMismatch,
    /// Proto proof is missing.
    MissingProof,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength { expected, got } => {
                write!(f, "invalid key length: got {got}, expected {expected}")
            }
            Self::UninitializedPrivKey => {
                write!(f, "ed25519 private key is missing its public-key suffix")
            }
            Self::InvalidAminoJson => write!(f, "invalid Amino JSON"),
            Self::WrongAminoType { expected, got } => {
                write!(f, "wrong Amino type: got {got:?}, expected {expected:?}")
            }
            Self::InvalidBase64 => write!(f, "invalid Amino base64"),
            Self::UnsupportedKeyType => write!(f, "unsupported public key type"),
            Self::NegativeProofTotal => write!(f, "negative Total"),
            Self::NegativeProofIndex => write!(f, "negative Index"),
            Self::InvalidLeafHash { got } => {
                write!(f, "expected LeafHash size to be 32, got {got}")
            }
            Self::TooManyAunts { got } => {
                write!(
                    f,
                    "expected no more than {} aunts, got {got}",
                    crate::MAX_AUNTS
                )
            }
            Self::InvalidAuntHash { index, got } => {
                write!(f, "expected Aunts#{index} size to be 32, got {got}")
            }
            Self::ProofLeafMismatch => write!(f, "invalid leaf hash"),
            Self::ProofRootMismatch => write!(f, "invalid root hash"),
            Self::MissingProof => write!(f, "nil proof"),
        }
    }
}

impl std::error::Error for Error {}
