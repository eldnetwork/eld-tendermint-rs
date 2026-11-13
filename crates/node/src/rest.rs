//! GET routes for the same methods `POST /` serves.
//!
//! Each path calls that JSON-RPC method and returns its envelope with `id` `-1`,
//! the Tendermint HTTP form of the call. A path that is not listed here is not
//! a REST request.

use serde_json::Value;

use crate::rpc::{self, NodeStatus};

/// The JSON-RPC body for this GET path, when the path is a REST route.
pub(crate) fn handle(target: &str, status: &NodeStatus) -> Option<Value> {
    let method = match path(target) {
        "/status" => "status",
        _ => return None,
    };
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": -1,
        "method": method,
    });
    Some(rpc::dispatch(&request, status))
}

fn path(target: &str) -> &str {
    target.split_once('?').map_or(target, |(path, _)| path)
}
