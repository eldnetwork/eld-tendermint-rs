//! Mempool gossip on the p2p switch.
//!
//! Channel `0x30` carries `tendermint.mempool.Message`. Each broadcast is one
//! `Txs` message with a single transaction. There is no height gate and no WAL.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::mempool::{self, Message};
use eld_tendermint_types::Tx;
use prost::Message as ProstMessage;

use crate::Error;
use crate::mempool::{App, Mempool};

/// `MempoolChannel`.
pub const MEMPOOL_CHANNEL: u8 = 0x30;

/// Channel list from `Reactor.GetChannels`. Register this before `add_peer`.
///
/// The send queue is the Go default of 1. `recv_message_capacity` is the encoded
/// size of one `Message` holding a single tx of `max_tx_bytes` bytes.
#[must_use]
pub fn channel_descriptors(max_tx_bytes: i64) -> Vec<ChannelDescriptor> {
    vec![ChannelDescriptor {
        id: MEMPOOL_CHANNEL,
        priority: 5,
        send_queue_capacity: 1,
        recv_message_capacity: recv_capacity(max_tx_bytes),
    }]
}

fn recv_capacity(max_tx_bytes: i64) -> usize {
    let width = usize::try_from(max_tx_bytes).unwrap_or(0);
    Message {
        sum: Some(mempool::message::Sum::Txs(mempool::Txs {
            txs: vec![vec![0u8; width]],
        })),
    }
    .encoded_len()
}

struct Inner<A: App> {
    mempool: Arc<Mutex<Mempool<A>>>,
    ids: HashMap<String, u16>,
    next_id: u16,
    sent: HashMap<String, HashSet<Vec<u8>>>,
}

/// Local v0 pool plus the peer ids used to gossip on a [`Switch`].
pub struct Reactor<A: App> {
    inner: Arc<Mutex<Inner<A>>>,
}

impl<A: App> Clone for Reactor<A> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<A: App + Send> Reactor<A> {
    #[must_use]
    pub fn new(mempool: Mempool<A>) -> Self {
        Self::from_shared(Arc::new(Mutex::new(mempool)))
    }

    /// Same pool another task already holds, such as the consensus node.
    #[must_use]
    pub fn from_shared(mempool: Arc<Mutex<Mempool<A>>>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                mempool,
                ids: HashMap::new(),
                next_id: 1,
                sent: HashMap::new(),
            })),
        }
    }

    /// `CheckTx` as the local node. Sender id `0`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Mempool::check_tx`].
    pub fn check_tx(&self, tx: &Tx) -> Result<(), Error> {
        let inner = lock(&self.inner);
        lock(&inner.mempool).check_tx(tx)
    }

    /// Every pooled tx, in arrival order.
    #[must_use]
    pub fn reap(&self) -> Vec<Tx> {
        let inner = lock(&self.inner);
        lock(&inner.mempool)
            .reap_max_bytes_max_gas(-1, -1)
            .as_slice()
            .to_vec()
    }

    /// Send pooled txs that each peer has not already sent us.
    pub fn poll(&self, switch: &Switch) {
        let mut inner = lock(&self.inner);
        if !lock(&inner.mempool).broadcasts() {
            return;
        }
        let peer_ids: Vec<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        for peer_id in &peer_ids {
            inner.sender_id(peer_id);
        }
        let pooled = lock(&inner.mempool).pooled();
        for peer_id in peer_ids {
            let sender_id = inner.ids[&peer_id];
            for (tx, senders) in &pooled {
                if senders.contains(&sender_id) {
                    continue;
                }
                if inner
                    .sent
                    .get(&peer_id)
                    .is_some_and(|have| have.contains(tx.as_bytes()))
                {
                    continue;
                }
                let bytes = encode_tx(tx);
                if switch.send(&peer_id, MEMPOOL_CHANNEL, &bytes) {
                    inner
                        .sent
                        .entry(peer_id.clone())
                        .or_default()
                        .insert(tx.as_bytes().to_vec());
                } else {
                    break;
                }
            }
        }
    }

    /// Decode one reactor message and `CheckTx` each tx.
    ///
    /// Returns `false` when `bytes` is not a `mempool.Message` or the sum is
    /// missing. The caller stops that peer. An empty `Txs` list, a cache hit,
    /// and an app rejection return `true`.
    pub fn handle(&self, peer_id: &str, _ch_id: u8, bytes: &[u8]) -> bool {
        let Ok(message) = Message::decode(bytes) else {
            return false;
        };
        let Some(mempool::message::Sum::Txs(txs)) = message.sum else {
            return false;
        };
        if txs.txs.is_empty() {
            return true;
        }
        let mut inner = lock(&self.inner);
        let sender_id = inner.sender_id(peer_id);
        for raw in txs.txs {
            let tx = Tx::new(raw);
            let _ = lock(&inner.mempool).check_tx_with_sender(&tx, sender_id);
        }
        true
    }
}

impl<A: App> Inner<A> {
    fn sender_id(&mut self, peer_id: &str) -> u16 {
        if let Some(id) = self.ids.get(peer_id) {
            return *id;
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        self.ids.insert(peer_id.to_owned(), id);
        id
    }
}

fn encode_tx(tx: &Tx) -> Vec<u8> {
    Message {
        sum: Some(mempool::message::Sum::Txs(mempool::Txs {
            txs: vec![tx.as_bytes().to_vec()],
        })),
    }
    .encode_to_vec()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
