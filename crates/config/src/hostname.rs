//! Host name for the default moniker.
//!
//! This module is the only `unsafe` in the workspace. RocksDB is opened from
//! `crates/store/src/db.rs`; that FFI lives in the `rocksdb` crate.

#![allow(unsafe_code)]

/// `getDefaultMoniker`: the host name, or `"anonymous"` when it cannot be read.
#[must_use]
pub fn default_moniker() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is a writable 256-byte buffer. `gethostname` writes at most
    // `buf.len()` bytes and NUL-terminates on success. A non-zero return means
    // the name was not written, and the caller uses `"anonymous"`.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return "anonymous".to_owned();
    }
    let end = buf.iter().position(|byte| *byte == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).unwrap_or_else(|_| "anonymous".to_owned())
}
