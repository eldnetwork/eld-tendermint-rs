//! Panic when an invariant does not hold.
//!
//! `.unwrap()` and `.expect()` are denied outside tests. This trait is for a
//! value that cannot fail, such as a length that fits in `usize`. A path that
//! can fail returns [`Result`] instead.

pub(crate) trait Ensured<T> {
    fn ensured(self, message: &str) -> T;
}

impl<T, E: std::fmt::Debug> Ensured<T> for Result<T, E> {
    fn ensured(self, message: &str) -> T {
        match self {
            Ok(value) => value,
            Err(err) => panic!("{message}: {err:?}"),
        }
    }
}

impl<T> Ensured<T> for Option<T> {
    fn ensured(self, message: &str) -> T {
        match self {
            Some(value) => value,
            None => panic!("{message}"),
        }
    }
}
