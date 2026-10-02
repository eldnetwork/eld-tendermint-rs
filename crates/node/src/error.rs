use std::fmt;

/// A startup failure. The process prints this and exits 1.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub(crate) fn fail(err: impl fmt::Display) -> Error {
    Error::new(err.to_string())
}
