//! `eld-tendermint start` loads one home and serves JSON-RPC.
//! `eld-tendermint unsafe-reset-all` wipes chain data for the next start.

mod app;
<<<<<<< HEAD
mod capacity_validator_key;
mod challenge_response;
=======
>>>>>>> parent of 3a9d3ed (Add capacity validator key to tendermint node, and load it on boot)
mod content_wait;
mod error;
mod node;
mod reset;
mod rpc;
mod rpc_json;
mod wait;
mod ws;

fn main() {
    if let Err(err) = node::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
