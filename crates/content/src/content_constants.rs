//! Content-channel size limits and how long the node waits for one reply.

use std::time::Duration;

/// Largest blob a push or response may carry. Pinboard messages stop at 1 MiB.
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;

/// Reassembled message cap. One max blob, plus protobuf tags and a content id.
pub const RECV_MESSAGE_CAPACITY: usize = MAX_BLOB_BYTES + 4096;

pub const CONTENT_REQUEST_WAIT: Duration = Duration::from_secs(10);
pub const CONTENT_CHALLENGE_WAIT: Duration = Duration::from_secs(15);

/// How long `content_push` waits for one receipt. The app asks every provider inside this budget.
pub const CONTENT_PUSH_WAIT: Duration = Duration::from_secs(30);
