//! One-line operator logs on stderr.
//!
//! Levels are `INFO`, `ERROR`, and `WARN`. There is no tracing subscriber.
//! A test may record the same lines with [`set_log_capture`].

use std::io::{self, Write};
use std::sync::{Arc, Mutex, OnceLock};

/// Operator log level. Debug is not printed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// Normal operator progress.
    Info,
    /// A failed dial, handshake, check, or block apply.
    Error,
    /// Reserved. Tendermint 0.34.24 does not log this level.
    Warn,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
        }
    }
}

static CAPTURE: OnceLock<Mutex<Option<Arc<Mutex<String>>>>> = OnceLock::new();

fn capture() -> &'static Mutex<Option<Arc<Mutex<String>>>> {
    CAPTURE.get_or_init(|| Mutex::new(None))
}

/// Record the same lines [`log_line`] writes to stderr.
///
/// `None` stops recording. The node process never calls this.
pub fn set_log_capture(buf: Option<Arc<Mutex<String>>>) {
    *capture().lock().unwrap_or_else(|err| err.into_inner()) = buf;
}

/// Uppercase hex for an address or hash field.
#[must_use]
pub fn upper_hex(bytes: &[u8]) -> String {
    hex::encode_upper(bytes)
}

/// Write one operator line to stderr and flush it.
///
/// The shape is `LEVEL module=<name> <message> key=value`. A message or field
/// value that contains a space is quoted. Newlines in a value become spaces.
pub fn log_line(level: Level, module: &str, message: &str, fields: &[(&str, &str)]) {
    let mut line = format!("{} module={module}", level.label());
    push_message(&mut line, message);
    for (key, value) in fields {
        push_field(&mut line, key, value);
    }
    let mut stderr = io::stderr().lock();
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
    if let Some(buf) = capture()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        let mut text = buf.lock().unwrap_or_else(|err| err.into_inner());
        text.push_str(&line);
        text.push('\n');
    }
}

fn push_message(line: &mut String, message: &str) {
    if message.is_empty() {
        return;
    }
    line.push(' ');
    push_token(line, message);
}

fn push_field(line: &mut String, key: &str, value: &str) {
    let flat = value.replace(['\n', '\r'], " ");
    line.push(' ');
    line.push_str(key);
    line.push('=');
    push_token(line, &flat);
}

fn push_token(line: &mut String, value: &str) {
    if value.contains(' ') || value.contains('"') {
        line.push('"');
        line.push_str(&value.replace('"', "'"));
        line.push('"');
    } else {
        line.push_str(value);
    }
}
