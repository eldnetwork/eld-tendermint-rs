//! `eld-tendermint start` loads one home and serves `status` and `health`.

mod app;
mod error;
mod node;
mod rpc;

fn main() {
    if let Err(err) = node::run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}
