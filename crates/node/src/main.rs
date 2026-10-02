//! `eld-tendermint start` loads one home and serves JSON-RPC.

mod app;
mod error;
mod node;
mod rpc;
mod wait;

fn main() {
    if let Err(err) = node::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
