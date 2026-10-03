//! `eld-tendermint start` loads one home and serves JSON-RPC.
//! `eld-tendermint unsafe-reset-all` wipes chain data for the next start.

mod app;
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
