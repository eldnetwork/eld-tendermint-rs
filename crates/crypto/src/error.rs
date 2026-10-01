use std::fmt;

/// Failures from key construction, Amino JSON, and proto conversion.
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
        }
    }
}

impl std::error::Error for Error {}
