#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `WriteMessage` of `RequestEcho` from `abci/types/messages_test.go`.

use eld_tendermint_abci::{read_message, write_message};
use eld_tendermint_proto::abci::RequestEcho;

/// Zigzag length of the 7-byte `RequestEcho{"Hello"}`, then those bytes.
/// An unsigned length prefix would start with `07`.
const FRAMED_HELLO: &str = "0e0a0548656c6c6f";

#[test]
fn request_echo_frame_matches_go_write_message() {
    let msg = RequestEcho {
        message: "Hello".to_owned(),
    };
    let mut buf = Vec::new();
    write_message(&mut buf, &msg).expect("write");
    assert_eq!(hex::encode(&buf), FRAMED_HELLO);
    assert_ne!(buf.first(), Some(&0x07));

    let decoded: RequestEcho = read_message(&mut buf.as_slice()).expect("read");
    assert_eq!(decoded, msg);
}
