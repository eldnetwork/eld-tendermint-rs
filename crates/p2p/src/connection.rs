//! `conn.MConnection`.
//!
//! Length-delimited `tendermint.p2p.Packet` messages on a [`crate::SecretConnection`].
//! A `PacketMsg` payload is at most 1024 bytes. Larger sends are split, and `eof` is
//! set on the last chunk. `on_receive` runs only after reassembly. There is no flow
//! monitor and no flush throttle: each packet is written immediately.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eld_tendermint_proto::p2p::{Packet, PacketMsg, PacketPing, PacketPong, packet};
use prost::Message;

use crate::error::Error;
use crate::secret_connection::{IoShutdown, SecretReader, SecretWriter};

const NUM_BATCH_PACKET_MSGS: usize = 10;

/// `conn.ChannelDescriptor`. `recv_message_capacity` is Go's `RecvMessageCapacity`.
#[derive(Clone, Debug)]
pub struct ChannelDescriptor {
    pub id: u8,
    pub priority: u32,
    pub send_queue_capacity: usize,
    pub recv_message_capacity: usize,
}

/// `conn.MConnConfig`. Rates and the flush throttle are not used.
#[derive(Clone, Debug)]
pub struct MConnConfig {
    pub max_packet_msg_payload_size: usize,
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    pub send_timeout: Duration,
}

impl Default for MConnConfig {
    fn default() -> Self {
        Self {
            max_packet_msg_payload_size: 1024,
            ping_interval: Duration::from_secs(60),
            pong_timeout: Duration::from_secs(45),
            send_timeout: Duration::from_secs(10),
        }
    }
}

enum Wake {
    Send,
    SendPong,
    PongReceived,
    Quit,
}

struct Shared {
    running: Arc<AtomicBool>,
    errored: AtomicBool,
    queues: HashMap<u8, Arc<SendQueue>>,
    wake_tx: mpsc::Sender<Wake>,
    send_timeout: Duration,
}

/// Bounded per-channel send queue. `SyncSender::send_timeout` is unstable on Rust 1.86.
struct SendQueue {
    cap: usize,
    buf: Mutex<VecDeque<Vec<u8>>>,
    cv: Condvar,
}

impl SendQueue {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            buf: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
        }
    }

    fn send_timeout(&self, msg: Vec<u8>, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
        loop {
            if buf.len() < self.cap {
                buf.push_back(msg);
                self.cv.notify_one();
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let (next, wait) = self
                .cv
                .wait_timeout(buf, deadline.saturating_duration_since(now))
                .unwrap_or_else(|err| err.into_inner());
            buf = next;
            if wait.timed_out() && buf.len() >= self.cap {
                return false;
            }
        }
    }

    fn try_send(&self, msg: Vec<u8>) -> bool {
        let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
        if buf.len() >= self.cap {
            return false;
        }
        buf.push_back(msg);
        self.cv.notify_one();
        true
    }

    fn try_recv(&self) -> Option<Vec<u8>> {
        let mut buf = self.buf.lock().unwrap_or_else(|err| err.into_inner());
        let msg = buf.pop_front();
        if msg.is_some() {
            self.cv.notify_one();
        }
        msg
    }
}

struct SendChannel {
    id: u8,
    priority: u32,
    queue: Arc<SendQueue>,
    /// `None` means idle. `Some` is the message currently being packetized, which may be empty.
    sending: Option<Vec<u8>>,
    recently_sent: i64,
}

struct RecvChannel {
    capacity: usize,
    recving: Vec<u8>,
}

/// Multiplexed connection. `send` queues by channel. A peer-fatal read stops this
/// connection and runs `on_error` once.
pub struct MConnection {
    shared: Arc<Shared>,
    shutdown: Arc<dyn IoShutdown>,
    send_thread: Mutex<Option<JoinHandle<()>>>,
    recv_thread: Mutex<Option<JoinHandle<()>>>,
}

impl MConnection {
    /// Start the send and receive threads.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadPingConfig`] when `pong_timeout >= ping_interval`,
    /// [`Error::InvalidChannel`] when a descriptor has priority or queue capacity `0`,
    /// [`Error::DuplicateChannel`] when two descriptors share an id, or an I/O error
    /// when a thread cannot be spawned.
    pub fn start<R, W, Sh, F, E>(
        reader: SecretReader<R>,
        writer: SecretWriter<W>,
        shutdown: Sh,
        descriptors: Vec<ChannelDescriptor>,
        on_receive: F,
        on_error: E,
        config: MConnConfig,
    ) -> Result<Self, Error>
    where
        R: Read + Send + 'static,
        W: Write + Send + 'static,
        Sh: IoShutdown + 'static,
        F: Fn(u8, Vec<u8>) + Send + 'static,
        E: Fn(Error) + Send + Sync + 'static,
    {
        if config.pong_timeout >= config.ping_interval {
            return Err(Error::BadPingConfig);
        }
        validate_descriptors(&descriptors)?;

        let (wake_tx, wake_rx) = mpsc::channel();
        let mut queues = HashMap::new();
        let mut send_channels = Vec::with_capacity(descriptors.len());
        let mut recv_channels = HashMap::new();
        for desc in &descriptors {
            let queue = Arc::new(SendQueue::new(desc.send_queue_capacity));
            queues.insert(desc.id, Arc::clone(&queue));
            send_channels.push(SendChannel {
                id: desc.id,
                priority: desc.priority,
                queue,
                sending: None,
                recently_sent: 0,
            });
            recv_channels.insert(
                desc.id,
                RecvChannel {
                    capacity: desc.recv_message_capacity,
                    recving: Vec::new(),
                },
            );
        }

        let running = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Shared {
            running: Arc::clone(&running),
            errored: AtomicBool::new(false),
            queues,
            wake_tx,
            send_timeout: config.send_timeout,
        });
        let shutdown: Arc<dyn IoShutdown> = Arc::new(shutdown);
        let on_error: Arc<dyn Fn(Error) + Send + Sync> = Arc::new(on_error);
        let max_payload = config.max_packet_msg_payload_size;
        let max_packet = max_packet_msg_size(max_payload);

        let session = Session {
            shared: Arc::clone(&shared),
            shutdown: Arc::clone(&shutdown),
            on_error,
        };
        let send_session = session.clone();
        let send_thread = thread::Builder::new()
            .name("mconn-send".to_owned())
            .spawn(move || {
                send_loop(
                    writer,
                    send_channels,
                    wake_rx,
                    send_session,
                    config.ping_interval,
                    config.pong_timeout,
                    max_payload,
                );
            })
            .map_err(Error::Io)?;

        let recv_thread =
            match thread::Builder::new()
                .name("mconn-recv".to_owned())
                .spawn(move || {
                    recv_loop(
                        reader,
                        recv_channels,
                        max_packet,
                        max_payload,
                        session,
                        on_receive,
                    );
                }) {
                Ok(handle) => handle,
                Err(err) => {
                    shared.running.store(false, Ordering::SeqCst);
                    let _ = shared.wake_tx.send(Wake::Quit);
                    let _ = shutdown.shutdown_io();
                    let _ = send_thread.join();
                    return Err(Error::Io(err));
                }
            };

        Ok(Self {
            shared,
            shutdown,
            send_thread: Mutex::new(Some(send_thread)),
            recv_thread: Mutex::new(Some(recv_thread)),
        })
    }

    /// `MConnection.Send`. Unknown channels and a full queue return `false` and leave the peer up.
    pub fn send(&self, ch_id: u8, msg: &[u8]) -> bool {
        self.enqueue(ch_id, msg, true)
    }

    /// `MConnection.TrySend`. Returns `false` immediately when that channel queue is full.
    pub fn try_send(&self, ch_id: u8, msg: &[u8]) -> bool {
        self.enqueue(ch_id, msg, false)
    }

    /// True until a peer-fatal error or [`Self::stop`].
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    pub(crate) fn running_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shared.running)
    }

    /// Stop the threads without joining them.
    ///
    /// The receive callback runs on the receive thread. Joining that thread from
    /// the callback would deadlock. [`Self::stop`] joins after this returns.
    pub fn close(&self) {
        self.shared.running.store(false, Ordering::SeqCst);
        let _ = self.shared.wake_tx.send(Wake::Quit);
        let _ = self.shutdown.shutdown_io();
    }

    /// Close the socket and join both threads.
    pub fn stop(&self) {
        self.close();
        let mut panicked = None;
        for slot in [&self.send_thread, &self.recv_thread] {
            let handle = slot.lock().unwrap_or_else(|err| err.into_inner()).take();
            if let Some(handle) = handle {
                if let Err(payload) = handle.join() {
                    panicked = Some(payload);
                }
            }
        }
        if let Some(payload) = panicked {
            std::panic::resume_unwind(payload);
        }
    }

    fn enqueue(&self, ch_id: u8, msg: &[u8], block: bool) -> bool {
        if !self.is_running() {
            return false;
        }
        let Some(queue) = self.shared.queues.get(&ch_id) else {
            return false;
        };
        let queued = if block {
            queue.send_timeout(msg.to_vec(), self.shared.send_timeout)
        } else {
            queue.try_send(msg.to_vec())
        };
        if queued {
            let _ = self.shared.wake_tx.send(Wake::Send);
        }
        queued
    }
}

impl Drop for MConnection {
    fn drop(&mut self) {
        self.stop();
    }
}

fn validate_descriptors(descriptors: &[ChannelDescriptor]) -> Result<(), Error> {
    let mut seen = HashMap::new();
    for desc in descriptors {
        if desc.priority == 0 || desc.send_queue_capacity == 0 {
            return Err(Error::InvalidChannel);
        }
        if seen.insert(desc.id, ()).is_some() {
            return Err(Error::DuplicateChannel { id: desc.id });
        }
    }
    Ok(())
}

fn max_packet_msg_size(max_payload: usize) -> usize {
    Packet {
        sum: Some(packet::Sum::PacketMsg(PacketMsg {
            channel_id: 0x01,
            eof: true,
            data: vec![0u8; max_payload],
        })),
    }
    .encoded_len()
}

struct Session {
    shared: Arc<Shared>,
    shutdown: Arc<dyn IoShutdown>,
    on_error: Arc<dyn Fn(Error) + Send + Sync>,
}

impl Clone for Session {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            shutdown: Arc::clone(&self.shutdown),
            on_error: Arc::clone(&self.on_error),
        }
    }
}

impl Session {
    fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }

    fn fail(&self, err: Error) {
        if self
            .shared
            .errored
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.shared.running.store(false, Ordering::SeqCst);
            let _ = self.shutdown.shutdown_io();
            let _ = self.shared.wake_tx.send(Wake::Quit);
            (self.on_error)(err);
        }
    }
}

fn recv_loop<R, F>(
    mut reader: SecretReader<R>,
    mut channels: HashMap<u8, RecvChannel>,
    max_packet: usize,
    max_payload: usize,
    session: Session,
    on_receive: F,
) where
    R: Read,
    F: Fn(u8, Vec<u8>),
{
    loop {
        if !session.is_running() {
            break;
        }
        let packet: Packet = match crate::secret_connection::read_msg(&mut reader, max_packet) {
            Ok(packet) => packet,
            Err(err) => {
                if session.is_running() {
                    session.fail(err);
                }
                break;
            }
        };
        match packet.sum {
            Some(packet::Sum::PacketPing(_)) => {
                let _ = session.shared.wake_tx.send(Wake::SendPong);
            }
            Some(packet::Sum::PacketPong(_)) => {
                let _ = session.shared.wake_tx.send(Wake::PongReceived);
            }
            Some(packet::Sum::PacketMsg(msg)) => {
                if let Err(err) = handle_packet_msg(&mut channels, msg, max_payload, &on_receive) {
                    session.fail(err);
                    break;
                }
            }
            None => {
                session.fail(Error::Proto("unknown packet type".to_owned()));
                break;
            }
        }
    }
}

fn handle_packet_msg<F>(
    channels: &mut HashMap<u8, RecvChannel>,
    msg: PacketMsg,
    max_payload: usize,
    on_receive: &F,
) -> Result<(), Error>
where
    F: Fn(u8, Vec<u8>),
{
    if msg.channel_id < 0 || msg.channel_id > i32::from(u8::MAX) {
        return Err(Error::UnknownChannel { id: msg.channel_id });
    }
    let id = u8::try_from(msg.channel_id).expect("channel id checked against u8");
    let Some(channel) = channels.get_mut(&id) else {
        return Err(Error::UnknownChannel { id: msg.channel_id });
    };
    if msg.data.len() > max_payload {
        return Err(Error::ChunkTooBig);
    }
    if let Some(bytes) = channel.push(msg)? {
        on_receive(id, bytes);
    }
    Ok(())
}

impl RecvChannel {
    fn push(&mut self, msg: PacketMsg) -> Result<Option<Vec<u8>>, Error> {
        let got = self.recving.len().saturating_add(msg.data.len());
        if got > self.capacity {
            return Err(Error::MessageExceedsCapacity {
                cap: self.capacity,
                got,
            });
        }
        self.recving.extend_from_slice(&msg.data);
        if msg.eof {
            Ok(Some(std::mem::take(&mut self.recving)))
        } else {
            Ok(None)
        }
    }
}

fn send_loop<W: Write>(
    mut writer: SecretWriter<W>,
    mut channels: Vec<SendChannel>,
    wake_rx: Receiver<Wake>,
    session: Session,
    ping_interval: Duration,
    pong_timeout: Duration,
    max_payload: usize,
) {
    let mut next_ping = Instant::now() + ping_interval;
    let mut pong_deadline: Option<Instant> = None;

    loop {
        if !session.is_running() {
            break;
        }
        if pong_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            match wake_rx.try_recv() {
                Ok(Wake::PongReceived) => {
                    pong_deadline = None;
                    continue;
                }
                Ok(wake) => {
                    if apply_wake(
                        wake,
                        &mut writer,
                        &mut channels,
                        &session,
                        &mut pong_deadline,
                        max_payload,
                    ) {
                        break;
                    }
                    continue;
                }
                Err(TryRecvError::Empty) => {
                    session.fail(Error::PongTimeout);
                    break;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }

        let wake_at = match pong_deadline {
            Some(deadline) if deadline < next_ping => deadline,
            _ => next_ping,
        };
        let wait = wake_at.saturating_duration_since(Instant::now());
        let wake = match wake_rx.recv_timeout(wait) {
            Ok(wake) => wake,
            Err(RecvTimeoutError::Timeout) => {
                if pong_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    session.fail(Error::PongTimeout);
                    break;
                }
                if Instant::now() >= next_ping {
                    match write_packet(&mut writer, &ping_packet()) {
                        Ok(()) => {
                            let now = Instant::now();
                            pong_deadline = Some(now + pong_timeout);
                            next_ping = now + ping_interval;
                        }
                        Err(err) => {
                            session.fail(err);
                            break;
                        }
                    }
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if apply_wake(
            wake,
            &mut writer,
            &mut channels,
            &session,
            &mut pong_deadline,
            max_payload,
        ) {
            break;
        }
    }
}

/// Returns true when the send loop should exit.
fn apply_wake<W: Write>(
    wake: Wake,
    writer: &mut SecretWriter<W>,
    channels: &mut [SendChannel],
    session: &Session,
    pong_deadline: &mut Option<Instant>,
    max_payload: usize,
) -> bool {
    match wake {
        Wake::Quit => true,
        Wake::PongReceived => {
            *pong_deadline = None;
            false
        }
        Wake::SendPong => match write_packet(writer, &pong_packet()) {
            Ok(()) => false,
            Err(err) => {
                session.fail(err);
                true
            }
        },
        Wake::Send => match send_some(writer, channels, &session.shared, max_payload) {
            Ok(()) => false,
            Err(err) => {
                session.fail(err);
                true
            }
        },
    }
}

fn send_some<W: Write>(
    writer: &mut SecretWriter<W>,
    channels: &mut [SendChannel],
    shared: &Shared,
    max_payload: usize,
) -> Result<(), Error> {
    let mut exhausted = false;
    for _ in 0..NUM_BATCH_PACKET_MSGS {
        exhausted = send_one(writer, channels, max_payload)?;
        if exhausted {
            break;
        }
    }
    if !exhausted {
        let _ = shared.wake_tx.send(Wake::Send);
    }
    Ok(())
}

/// Returns true when every channel queue is idle.
fn send_one<W: Write>(
    writer: &mut SecretWriter<W>,
    channels: &mut [SendChannel],
    max_payload: usize,
) -> Result<bool, Error> {
    let mut best: Option<usize> = None;
    let mut best_ratio = f32::MAX;
    for (i, channel) in channels.iter_mut().enumerate() {
        if !channel.pull_pending() {
            continue;
        }
        let ratio = channel.recently_sent as f32 / channel.priority as f32;
        if ratio < best_ratio {
            best_ratio = ratio;
            best = Some(i);
        }
    }
    let Some(index) = best else {
        return Ok(true);
    };
    let n = channels[index].write_next(writer, max_payload)?;
    channels[index].recently_sent += i64::try_from(n).unwrap_or(i64::MAX);
    Ok(false)
}

impl SendChannel {
    fn pull_pending(&mut self) -> bool {
        if self.sending.is_none() {
            match self.queue.try_recv() {
                Some(bytes) => self.sending = Some(bytes),
                None => return false,
            }
        }
        true
    }

    fn write_next<W: Write>(
        &mut self,
        writer: &mut SecretWriter<W>,
        max_payload: usize,
    ) -> Result<usize, Error> {
        let sending = self.sending.take().expect("channel has a pending message");
        let n = sending.len().min(max_payload);
        let eof = sending.len() <= max_payload;
        let data = sending[..n].to_vec();
        if !eof {
            self.sending = Some(sending[n..].to_vec());
        }
        let packet = Packet {
            sum: Some(packet::Sum::PacketMsg(PacketMsg {
                channel_id: i32::from(self.id),
                eof,
                data,
            })),
        };
        let bytes = packet.encode_length_delimited_to_vec();
        let written = bytes.len();
        write_packet_bytes(writer, &bytes)?;
        Ok(written)
    }
}

fn ping_packet() -> Packet {
    Packet {
        sum: Some(packet::Sum::PacketPing(PacketPing {})),
    }
}

fn pong_packet() -> Packet {
    Packet {
        sum: Some(packet::Sum::PacketPong(PacketPong {})),
    }
}

fn write_packet<W: Write>(writer: &mut SecretWriter<W>, packet: &Packet) -> Result<(), Error> {
    write_packet_bytes(writer, &packet.encode_length_delimited_to_vec())
}

fn write_packet_bytes<W: Write>(writer: &mut SecretWriter<W>, bytes: &[u8]) -> Result<(), Error> {
    writer.write_all(bytes).map_err(Error::Io)?;
    writer.flush().map_err(Error::Io)?;
    Ok(())
}
