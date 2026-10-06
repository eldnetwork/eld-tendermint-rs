//! Tendermint 0.17 ABCI socket client.
//!
//! Frames use `WriteMessage`'s zigzag length prefix. Each call sends one request
//! and a flush. There is no server and no consensus loop.

mod client;
mod ensured;
mod error;
mod protoio;

pub use client::SocketClient;
pub use error::Error;
pub use protoio::{read_message, write_message};
