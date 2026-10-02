//! `p2p.Switch` without the dial loop.
//!
//! A reactor is a name, a channel list, and an `on_receive` callback. `add_peer` starts
//! an [`MConnection`](crate::MConnection) on a secret connection using the channels
//! registered so far. There is no PEX, address book, or accept loop.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::connection::{ChannelDescriptor, MConnConfig, MConnection};
use crate::error::Error;
use crate::secret_connection::{SecretConnection, SplitIo};

type ReceiveCb = Arc<dyn Fn(&str, u8, Vec<u8>) + Send + Sync>;

struct PeerSlot {
    id: String,
    remote_addr: String,
    running: Arc<AtomicBool>,
    conn: Box<dyn PeerConn>,
}

trait PeerConn: Send {
    fn send(&self, ch_id: u8, bytes: &[u8]) -> bool;
    fn close(&self);
    fn stop(&self);
}

impl PeerConn for MConnection {
    fn send(&self, ch_id: u8, bytes: &[u8]) -> bool {
        Self::send(self, ch_id, bytes)
    }

    fn close(&self) {
        Self::close(self);
    }

    fn stop(&self) {
        Self::stop(self);
    }
}

/// One connected peer. `remote_addr` is empty until a dialed address exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub id: String,
    pub remote_addr: String,
}

struct Inner {
    by_ch: HashMap<u8, ReceiveCb>,
    descriptors: Vec<ChannelDescriptor>,
    peers: Vec<PeerSlot>,
}

/// Registers reactors and runs one [`MConnection`](crate::MConnection) per peer.
pub struct Switch {
    inner: Mutex<Inner>,
}

impl Switch {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                by_ch: HashMap::new(),
                descriptors: Vec::new(),
                peers: Vec::new(),
            }),
        }
    }

    /// Register a reactor. Channel ids must be unique across the switch.
    ///
    /// `on_receive` is `peer_id`, channel id, and the reassembled bytes. Descriptors
    /// are snapshotted when a peer is added. A later `add_reactor` does not attach
    /// channels to peers that are already running.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DuplicateChannel`] when `id` is already registered, and
    /// [`Error::InvalidChannel`] when priority or the send queue capacity is zero.
    pub fn add_reactor<F>(
        &self,
        name: impl Into<String>,
        channels: Vec<ChannelDescriptor>,
        on_receive: F,
    ) -> Result<(), Error>
    where
        F: Fn(&str, u8, Vec<u8>) + Send + Sync + 'static,
    {
        let _name = name.into();
        let mut inner = lock(&self.inner);
        let mut seen = HashMap::new();
        for desc in &channels {
            if desc.priority == 0 || desc.send_queue_capacity == 0 {
                return Err(Error::InvalidChannel);
            }
            if inner.by_ch.contains_key(&desc.id) || seen.insert(desc.id, ()).is_some() {
                return Err(Error::DuplicateChannel { id: desc.id });
            }
        }
        let callback: ReceiveCb = Arc::new(on_receive);
        for desc in &channels {
            inner.by_ch.insert(desc.id, Arc::clone(&callback));
            inner.descriptors.push(desc.clone());
        }
        Ok(())
    }

    /// Start an `MConnection` for `node_id` on the secret connection.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`MConnection::start`](crate::MConnection::start)
    /// and [`SecretConnection::split`](crate::SecretConnection::split).
    pub fn add_peer<S>(
        &self,
        conn: SecretConnection<S>,
        node_id: impl Into<String>,
    ) -> Result<(), Error>
    where
        S: SplitIo,
    {
        let node_id = node_id.into();
        let (descriptors, routes) = {
            let inner = lock(&self.inner);
            let routes = inner
                .descriptors
                .iter()
                .filter_map(|desc| {
                    inner
                        .by_ch
                        .get(&desc.id)
                        .map(|callback| (desc.id, Arc::clone(callback)))
                })
                .collect::<HashMap<_, _>>();
            (inner.descriptors.clone(), routes)
        };

        let (reader, writer, shutdown) = conn.split()?;
        let peer_id = node_id.clone();
        let on_receive = move |ch_id: u8, bytes: Vec<u8>| {
            if let Some(callback) = routes.get(&ch_id) {
                callback(&peer_id, ch_id, bytes);
            }
        };
        let conn = MConnection::start(
            reader,
            writer,
            shutdown,
            descriptors,
            on_receive,
            |_| {},
            MConnConfig::default(),
        )?;
        let running = conn.running_flag();
        lock(&self.inner).peers.push(PeerSlot {
            id: node_id,
            remote_addr: String::new(),
            running,
            conn: Box::new(conn),
        });
        Ok(())
    }

    /// Queue `bytes` on `ch_id` for one running peer.
    ///
    /// Returns `false` when the peer is unknown, the peer has stopped, or the
    /// channel is not registered. The peer stays up.
    pub fn send(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let inner = lock(&self.inner);
        let Some(peer) = inner
            .peers
            .iter()
            .find(|peer| peer.id == peer_id && peer.running.load(Ordering::SeqCst))
        else {
            return false;
        };
        peer.conn.send(ch_id, bytes)
    }

    /// Shut down one peer without joining from the caller.
    ///
    /// The connection threads exit on their own. [`Self::stop`] joins them.
    /// Unknown ids are ignored. Other peers stay up.
    pub fn stop_peer(&self, peer_id: &str) {
        let inner = lock(&self.inner);
        if let Some(peer) = inner.peers.iter().find(|peer| peer.id == peer_id) {
            peer.running.store(false, Ordering::SeqCst);
            peer.conn.close();
        }
    }

    /// Queue `bytes` on `ch_id` for every running peer.
    ///
    /// Returns `false` when any peer rejects the send. An unknown channel rejects
    /// the send and leaves the peer up. Returns `true` when there are no peers.
    pub fn broadcast(&self, ch_id: u8, bytes: &[u8]) -> bool {
        let inner = lock(&self.inner);
        inner
            .peers
            .iter()
            .filter(|peer| peer.running.load(Ordering::SeqCst))
            .all(|peer| peer.conn.send(ch_id, bytes))
    }

    /// Running peers. A peer that hit a fatal receive error is omitted.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerInfo> {
        let inner = lock(&self.inner);
        inner
            .peers
            .iter()
            .filter(|peer| peer.running.load(Ordering::SeqCst))
            .map(|peer| PeerInfo {
                id: peer.id.clone(),
                remote_addr: peer.remote_addr.clone(),
            })
            .collect()
    }

    /// Shut down every peer and join its threads.
    pub fn stop(&self) {
        let peers = std::mem::take(&mut lock(&self.inner).peers);
        for peer in peers {
            peer.running.store(false, Ordering::SeqCst);
            peer.conn.stop();
        }
    }
}

impl Default for Switch {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Switch {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
