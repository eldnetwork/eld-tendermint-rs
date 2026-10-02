//! Tendermint 0.34 v0 blockchain reactor.
//!
//! Fast sync on channel `0x40`. A peer taller than this node is asked for the
//! next blocks, at most 20 at a time. Each block is applied as soon as it is
//! the next height. The commit inside block H is checked against block H-1.

mod pool;
mod reactor;

pub use reactor::{BLOCKCHAIN_CHANNEL, Reactor, channel_descriptors};
