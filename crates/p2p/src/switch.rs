//! `p2p.Switch` with dial and accept.
//!
//! A reactor is a name, a channel list, and an `on_receive` callback. `add_peer` starts
//! an [`MConnection`](crate::MConnection) on a secret connection using the channels
//! registered so far. `listen` and `dial_persistent` run that handshake on TCP.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::address::{NetAddress, parse_persistent_peers};
use crate::connection::{ChannelDescriptor, MConnConfig, MConnection};
use crate::error::Error;
use crate::node_key::NodeKey;
use crate::secret_connection::{SecretConnection, SplitIo, make_secret_connection};

type ReceiveCb = Arc<dyn Fn(&str, u8, Vec<u8>) + Send + Sync>;

const DIAL_TIMEOUT: Duration = Duration::from_secs(3);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

struct PeerSlot {
    id: String,
    remote_addr: String,
    running: Arc<AtomicBool>,
    conn: Box<dyn PeerConn>,
}

trait PeerConn: Send {
    fn send(&self, ch_id: u8, bytes: &[u8]) -> bool;
    fn try_send(&self, ch_id: u8, bytes: &[u8]) -> bool;
    fn close(&self);
    fn stop(&self);
}

impl PeerConn for MConnection {
    fn send(&self, ch_id: u8, bytes: &[u8]) -> bool {
        Self::send(self, ch_id, bytes)
    }

    fn try_send(&self, ch_id: u8, bytes: &[u8]) -> bool {
        Self::try_send(self, ch_id, bytes)
    }

    fn close(&self) {
        Self::close(self)
    }

    fn stop(&self) {
        Self::stop(self)
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
    dialing: HashSet<String>,
}

struct State {
    inner: Mutex<Inner>,
    listener: Mutex<Option<TcpListener>>,
    local_addr: Mutex<Option<SocketAddr>>,
    accept_thread: Mutex<Option<JoinHandle<()>>>,
    accepting: AtomicBool,
}

/// Registers reactors and runs one [`MConnection`](crate::MConnection) per peer.
pub struct Switch {
    state: Arc<State>,
}

impl Switch {
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(State {
                inner: Mutex::new(Inner {
                    by_ch: HashMap::new(),
                    descriptors: Vec::new(),
                    peers: Vec::new(),
                    dialing: HashSet::new(),
                }),
                listener: Mutex::new(None),
                local_addr: Mutex::new(None),
                accept_thread: Mutex::new(None),
                accepting: AtomicBool::new(false),
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
        let mut inner = lock(&self.state.inner);
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

    /// Bind `laddr` (`host:port` or `tcp://host:port`) and accept secret connections.
    ///
    /// Port `0` picks a free port. The returned address is the bound socket.
    /// Register reactors before the peer is accepted; `add_peer` snapshots them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNetAddress`] when `laddr` is not a socket address,
    /// or [`Error::Io`] when the bind fails.
    pub fn listen(&self, node_key: &NodeKey, laddr: &str) -> Result<SocketAddr, Error> {
        let addr = parse_listen(laddr)?;
        let listener = TcpListener::bind(addr).map_err(Error::Io)?;
        let bound = listener.local_addr().map_err(Error::Io)?;
        *lock(&self.state.listener) = Some(listener);
        *lock(&self.state.local_addr) = Some(bound);
        self.state.accepting.store(true, Ordering::SeqCst);
        let state = Arc::clone(&self.state);
        let key = node_key.clone();
        let handle = thread::spawn(move || accept_loop(state, key));
        *lock(&self.state.accept_thread) = Some(handle);
        Ok(bound)
    }

    /// Dial every `ID@host:port` in a comma-separated `persistent_peers` string.
    ///
    /// An id mismatch or a refused connection skips that peer and continues.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidNetAddress`] when an entry does not parse.
    pub fn dial_persistent(&self, node_key: &NodeKey, peers: &str) -> Result<(), Error> {
        let addrs = parse_persistent_peers(peers)?;
        for addr in addrs {
            let _ = self.dial_address(node_key, &addr);
        }
        Ok(())
    }

    /// Dial one address. The secret-connection id must equal `addr.id`.
    ///
    /// Already-connected and in-progress ids return `Ok` and do not open a second socket.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IdMismatch`] when the handshake id differs, or [`Error::Io`]
    /// when the TCP connect or handshake fails.
    pub fn dial_address(&self, node_key: &NodeKey, addr: &NetAddress) -> Result<(), Error> {
        if self.is_connected_or_dialing(&addr.id) {
            return Ok(());
        }
        {
            let mut inner = lock(&self.state.inner);
            if connected_or_dialing(&inner, &addr.id) {
                return Ok(());
            }
            inner.dialing.insert(addr.id.clone());
        }
        let result = self.dial_locked(node_key, addr);
        lock(&self.state.inner).dialing.remove(&addr.id);
        result
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
        insert_peer(&self.state, conn, node_id.into(), String::new())
    }

    /// Queue `bytes` on `ch_id` for one running peer.
    ///
    /// Returns `false` when the peer is unknown, the peer has stopped, or the
    /// channel is not registered. The peer stays up. Waits up to the connection
    /// send timeout when that channel queue is full.
    pub fn send(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let inner = lock(&self.state.inner);
        let Some(peer) = inner
            .peers
            .iter()
            .find(|peer| peer.id == peer_id && peer.running.load(Ordering::SeqCst))
        else {
            return false;
        };
        peer.conn.send(ch_id, bytes)
    }

    /// [`Self::send`] that returns `false` immediately when the channel queue is full.
    ///
    /// A reactor poll holds its own lock while it gossips. Waiting on a full queue
    /// would block the receive thread that drains it.
    pub fn try_send(&self, peer_id: &str, ch_id: u8, bytes: &[u8]) -> bool {
        let inner = lock(&self.state.inner);
        let Some(peer) = inner
            .peers
            .iter()
            .find(|peer| peer.id == peer_id && peer.running.load(Ordering::SeqCst))
        else {
            return false;
        };
        peer.conn.try_send(ch_id, bytes)
    }

    /// Shut down one peer without joining from the caller.
    ///
    /// The connection threads exit on their own. [`Self::stop`] joins them.
    /// Unknown ids are ignored. Other peers stay up.
    pub fn stop_peer(&self, peer_id: &str) {
        let inner = lock(&self.state.inner);
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
        let inner = lock(&self.state.inner);
        inner
            .peers
            .iter()
            .filter(|peer| peer.running.load(Ordering::SeqCst))
            .all(|peer| peer.conn.send(ch_id, bytes))
    }

    /// `true` after [`Self::listen`] until [`Self::stop`].
    #[must_use]
    pub fn is_listening(&self) -> bool {
        self.state.accepting.load(Ordering::SeqCst)
    }

    /// Bound `tcp://` addresses. Empty until [`Self::listen`] returns.
    #[must_use]
    pub fn listeners(&self) -> Vec<String> {
        lock(&self.state.local_addr)
            .map(|addr| format!("tcp://{addr}"))
            .into_iter()
            .collect()
    }

    /// Running peers. A peer that hit a fatal receive error is omitted.
    #[must_use]
    pub fn peers(&self) -> Vec<PeerInfo> {
        let inner = lock(&self.state.inner);
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

    /// Shut down the listener, every peer, and join their threads.
    pub fn stop(&self) {
        self.state.accepting.store(false, Ordering::SeqCst);
        if let Some(addr) = *lock(&self.state.local_addr) {
            let _ = TcpStream::connect_timeout(&wake_addr(addr), Duration::from_millis(200));
        }
        if let Some(handle) = lock(&self.state.accept_thread).take() {
            let _ = handle.join();
        }
        *lock(&self.state.listener) = None;
        let peers = std::mem::take(&mut lock(&self.state.inner).peers);
        for peer in peers {
            peer.running.store(false, Ordering::SeqCst);
            peer.conn.stop();
        }
    }

    fn is_connected_or_dialing(&self, id: &str) -> bool {
        connected_or_dialing(&lock(&self.state.inner), id)
    }

    fn dial_locked(&self, node_key: &NodeKey, addr: &NetAddress) -> Result<(), Error> {
        let stream =
            TcpStream::connect_timeout(&addr.socket_addr(), DIAL_TIMEOUT).map_err(Error::Io)?;
        let conn = handshake(stream, &node_key.priv_key)?;
        let got = hex::encode(conn.remote_pub_key().address());
        if got != addr.id {
            return Err(Error::IdMismatch {
                expected: addr.id.clone(),
                got,
            });
        }
        insert_peer(
            &self.state,
            conn,
            addr.id.clone(),
            addr.socket_addr().to_string(),
        )
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

fn accept_loop(state: Arc<State>, node_key: NodeKey) {
    loop {
        if !state.accepting.load(Ordering::SeqCst) {
            break;
        }
        let listener = {
            let guard = lock(&state.listener);
            match guard.as_ref() {
                Some(listener) => match listener.try_clone() {
                    Ok(listener) => listener,
                    Err(_) => break,
                },
                None => break,
            }
        };
        match listener.accept() {
            Ok((stream, peer)) => {
                if !state.accepting.load(Ordering::SeqCst) {
                    break;
                }
                let remote = peer.to_string();
                if let Ok(conn) = handshake(stream, &node_key.priv_key) {
                    let id = hex::encode(conn.remote_pub_key().address());
                    let _ = insert_peer(&state, conn, id, remote);
                }
            }
            Err(_) => {
                if !state.accepting.load(Ordering::SeqCst) {
                    break;
                }
            }
        }
    }
}

fn handshake(
    stream: TcpStream,
    key: &eld_tendermint_crypto::PrivKey,
) -> Result<SecretConnection<TcpStream>, Error> {
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(Error::Io)?;
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(Error::Io)?;
    let conn = make_secret_connection(stream, key)?;
    conn.clear_socket_timeouts().map_err(Error::Io)?;
    Ok(conn)
}

fn insert_peer<S>(
    state: &State,
    conn: SecretConnection<S>,
    node_id: String,
    remote_addr: String,
) -> Result<(), Error>
where
    S: SplitIo,
{
    if already_connected(&lock(&state.inner), &node_id) {
        return Ok(());
    }
    let (descriptors, routes) = {
        let inner = lock(&state.inner);
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
    let started = MConnection::start(
        reader,
        writer,
        shutdown,
        descriptors,
        on_receive,
        |_| {},
        MConnConfig::default(),
    )?;
    let running = started.running_flag();
    let mut inner = lock(&state.inner);
    if already_connected(&inner, &node_id) {
        drop(inner);
        started.stop();
        return Ok(());
    }
    inner.peers.push(PeerSlot {
        id: node_id,
        remote_addr,
        running,
        conn: Box::new(started),
    });
    Ok(())
}

fn connected_or_dialing(inner: &Inner, id: &str) -> bool {
    inner.dialing.contains(id) || already_connected(inner, id)
}

fn already_connected(inner: &Inner, id: &str) -> bool {
    inner
        .peers
        .iter()
        .any(|peer| peer.id == id && peer.running.load(Ordering::SeqCst))
}

fn parse_listen(laddr: &str) -> Result<SocketAddr, Error> {
    let laddr = laddr.trim().strip_prefix("tcp://").unwrap_or(laddr.trim());
    laddr
        .parse::<SocketAddr>()
        .map_err(|_| Error::InvalidNetAddress {
            addr: laddr.to_owned(),
        })
}

fn wake_addr(addr: SocketAddr) -> SocketAddr {
    if addr.ip().is_unspecified() {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), addr.port())
    } else {
        addr
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
