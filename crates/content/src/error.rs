//! The app refused a content message. The peer stays connected.

use std::fmt;

/// The app did not accept a message from the content channel.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// The app refused the message. The reactor sends no reply.
    Rejected { reason: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected { reason } => f.write_str(reason),
        }
    }
}

impl std::error::Error for Error {}
