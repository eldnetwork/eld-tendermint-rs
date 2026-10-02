//! PEX reactor. `PexRequest` and `PexAddrs` on channel `0x00`.
//!
//! No seed mode and no `ensurePeersRoutine`. A `PexAddrs` list is accepted even
//! when this node did not send `PexRequest`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_proto::p2p::{self, Message};
use prost::Message as ProstMessage;

use crate::addrbook::AddrBook;
use crate::address::NetAddress;
use crate::connection::ChannelDescriptor;
use crate::node_key::NodeKey;
use crate::switch::Switch;

/// `PexChannel`.
pub const PEX_CHANNEL: u8 = 0x00;

/// `maxGetSelection`.
pub const MAX_GET_SELECTION: usize = 250;

const MAX_ADDRESS_SIZE: usize = 256;

/// Channel list from `Reactor.GetChannels`. Register this before `add_peer`.
#[must_use]
pub fn channel_descriptors() -> Vec<ChannelDescriptor> {
    vec![ChannelDescriptor {
        id: PEX_CHANNEL,
        priority: 1,
        send_queue_capacity: 10,
        recv_message_capacity: MAX_ADDRESS_SIZE * MAX_GET_SELECTION,
    }]
}

struct Inner {
    book: AddrBook,
    node_key: NodeKey,
}

/// Address book plus the node key used to dial addresses learned from peers.
pub struct Reactor {
    inner: Arc<Mutex<Inner>>,
}

impl Clone for Reactor {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Reactor {
    #[must_use]
    pub fn new(book: AddrBook, node_key: NodeKey) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner { book, node_key })),
        }
    }

    /// Stored address with this id, if the book has it.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<NetAddress> {
        lock(&self.inner)
            .book
            .get(id)
            .map(|known| known.addr.clone())
    }

    /// Dial book addresses that are not already connected.
    pub fn poll(&self, switch: &Switch) {
        let connected = connected_ids(switch);
        let (addrs, key) = {
            let inner = lock(&self.inner);
            let addrs = inner
                .book
                .addresses()
                .into_iter()
                .filter(|addr| !connected.iter().any(|id| id == &addr.id))
                .collect::<Vec<_>>();
            (addrs, inner.node_key.clone())
        };
        for addr in addrs {
            let _ = switch.dial_address(&key, &addr);
        }
    }

    /// Decode one PEX message.
    ///
    /// Returns `false` when `bytes` is not a `p2p.Message` or the sum is missing.
    /// The caller stops that peer. `PexRequest` is answered with at most
    /// [`MAX_GET_SELECTION`] addresses. `PexAddrs` is added to the book and dialed.
    pub fn handle(&self, switch: &Switch, peer_id: &str, bytes: &[u8]) -> bool {
        let Ok(message) = Message::decode(bytes) else {
            return false;
        };
        let Some(sum) = message.sum else {
            return false;
        };
        match sum {
            p2p::message::Sum::PexRequest(_) => {
                let addrs = {
                    let inner = lock(&self.inner);
                    inner
                        .book
                        .addresses()
                        .into_iter()
                        .take(MAX_GET_SELECTION)
                        .collect::<Vec<_>>()
                };
                let bytes = encode_addrs(&addrs);
                let _ = switch.send(peer_id, PEX_CHANNEL, &bytes);
            }
            p2p::message::Sum::PexAddrs(msg) => {
                let src = source_addr(switch, peer_id);
                let mut to_dial = Vec::new();
                {
                    let mut inner = lock(&self.inner);
                    for proto in &msg.addrs {
                        let Ok(addr) = NetAddress::from_proto(proto) else {
                            continue;
                        };
                        if addr.id == peer_id {
                            continue;
                        }
                        inner.book.add(addr.clone(), src.clone());
                        to_dial.push(addr);
                    }
                }
                let key = lock(&self.inner).node_key.clone();
                let connected = connected_ids(switch);
                for addr in to_dial {
                    if connected.iter().any(|id| id == &addr.id) {
                        continue;
                    }
                    let _ = switch.dial_address(&key, &addr);
                }
            }
        }
        true
    }
}

fn connected_ids(switch: &Switch) -> Vec<String> {
    switch.peers().into_iter().map(|peer| peer.id).collect()
}

fn source_addr(switch: &Switch, peer_id: &str) -> NetAddress {
    let socket = switch
        .peers()
        .into_iter()
        .find(|peer| peer.id == peer_id)
        .and_then(|peer| peer.remote_addr.parse::<SocketAddr>().ok());
    match socket {
        Some(socket) => NetAddress {
            id: peer_id.to_owned(),
            ip: socket.ip(),
            port: socket.port(),
        },
        None => NetAddress {
            id: peer_id.to_owned(),
            ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: 0,
        },
    }
}

fn encode_addrs(addrs: &[NetAddress]) -> Vec<u8> {
    Message {
        sum: Some(p2p::message::Sum::PexAddrs(p2p::PexAddrs {
            addrs: addrs.iter().map(NetAddress::to_proto).collect(),
        })),
    }
    .encode_to_vec()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
