//! v0 blockchain reactor on channel `0x40`.
//!
//! `poll` sends `StatusRequest` the first time a peer appears, then
//! `BlockRequest` for a peer that is taller than this store. `handle` answers
//! requests and applies a block only at `store.height() + 1`. A bad commit or a
//! height outside the requested window returns `false` so the switch stops that
//! peer. The block is not saved.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_p2p::{ChannelDescriptor, Switch};
use eld_tendermint_proto::blockchain::{self, Message};
use eld_tendermint_state::{App, State, StateStore, apply_block, validate_block};
use eld_tendermint_store::{BlockStore, Db};
use eld_tendermint_types::{BLOCK_PART_SIZE_BYTES, Block, BlockId, Commit, CommitSig};
use prost::Message as ProstMessage;

use crate::pool::{Incoming, Pool};

/// `BlockchainChannel`.
pub const BLOCKCHAIN_CHANNEL: u8 = 0x40;

/// `MaxBlockSizeBytes` plus the Go `BlockResponse` prefix (4 + 1).
const RECV_MESSAGE_CAPACITY: usize = 104_857_600 + 5;

/// Channel list from `Reactor.GetChannels`. Register this before `add_peer`.
#[must_use]
pub fn channel_descriptors() -> Vec<ChannelDescriptor> {
    vec![ChannelDescriptor {
        id: BLOCKCHAIN_CHANNEL,
        priority: 5,
        send_queue_capacity: 1000,
        recv_message_capacity: RECV_MESSAGE_CAPACITY,
    }]
}

struct Inner<A: App, D: Db> {
    blocks: Arc<BlockStore<D>>,
    states: Arc<StateStore<D>>,
    state: State,
    app: A,
    pool: Pool,
}

/// Fast-sync pool on a [`Switch`].
pub struct Reactor<A: App, D: Db> {
    inner: Arc<Mutex<Inner<A, D>>>,
}

impl<A: App, D: Db> Clone for Reactor<A, D> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<A: App, D: Db> Reactor<A, D> {
    #[must_use]
    pub fn new(
        blocks: Arc<BlockStore<D>>,
        states: Arc<StateStore<D>>,
        state: State,
        app: A,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                blocks,
                states,
                state,
                app,
                pool: Pool::new(),
            })),
        }
    }

    /// Block-store height. An empty store is 0.
    #[must_use]
    pub fn height(&self) -> i64 {
        lock(&self.inner).blocks.height()
    }

    /// App hash of the state last applied by this reactor.
    #[must_use]
    pub fn app_hash(&self) -> Vec<u8> {
        lock(&self.inner).state.app_hash.clone()
    }

    /// `BlockRequest` messages this reactor has queued.
    #[must_use]
    pub fn block_requests_sent(&self) -> u64 {
        lock(&self.inner).pool.requests_sent()
    }

    /// Height advertised by `peer_id`, once a `StatusResponse` has arrived.
    #[must_use]
    pub fn peer_height(&self, peer_id: &str) -> Option<i64> {
        lock(&self.inner).pool.peer(peer_id).map(|peer| peer.height)
    }

    /// Send status and block requests for the peers on `switch`.
    pub fn poll(&self, switch: &Switch) {
        let mut inner = lock(&self.inner);
        let live: HashSet<String> = switch.peers().into_iter().map(|peer| peer.id).collect();
        inner.pool.forget_absent(&live);
        let greet: Vec<String> = live
            .iter()
            .filter(|id| inner.pool.greet(id))
            .cloned()
            .collect();
        let our_height = inner.blocks.height();
        let requests = inner.pool.requests(our_height);
        for (peer_id, height) in &requests {
            inner.pool.mark_requested(peer_id, *height);
        }
        let ready = inner.apply_ready();
        drop(inner);

        for peer_id in greet {
            let _ = switch.send(&peer_id, BLOCKCHAIN_CHANNEL, &encode(status_request()));
        }
        for (peer_id, height) in requests {
            if !switch.send(&peer_id, BLOCKCHAIN_CHANNEL, &encode(block_request(height))) {
                lock(&self.inner).pool.cancel_request(&peer_id, height);
            }
        }
        for peer_id in ready {
            switch.stop_peer(&peer_id);
        }
    }

    /// Decode one reactor message.
    ///
    /// Returns `false` when the bytes are not a `blockchain.Message`, a height
    /// is negative, a block's commit does not match the previous block, or the
    /// height was not requested. The caller stops that peer.
    pub fn handle(&self, switch: &Switch, peer_id: &str, _ch_id: u8, bytes: &[u8]) -> bool {
        let Ok(message) = Message::decode(bytes) else {
            return false;
        };
        let Some(sum) = message.sum else {
            return false;
        };
        let mut inner = lock(&self.inner);
        let outcome = match sum {
            blockchain::message::Sum::StatusRequest(_) => {
                let response = status_response(inner.blocks.height(), inner.blocks.base());
                Outcome::Reply(encode(response))
            }
            blockchain::message::Sum::StatusResponse(status) => {
                if status.height < 0 || status.base < 0 {
                    Outcome::Stop
                } else {
                    inner.pool.set_range(peer_id, status.base, status.height);
                    Outcome::Ok
                }
            }
            blockchain::message::Sum::BlockRequest(request) => {
                if request.height < 0 {
                    Outcome::Stop
                } else {
                    Outcome::Reply(encode(inner.block_answer(request.height)))
                }
            }
            blockchain::message::Sum::NoBlockResponse(missing) => {
                if missing.height < 0 {
                    Outcome::Stop
                } else {
                    inner.pool.take_no_block(peer_id, missing.height);
                    Outcome::Ok
                }
            }
            blockchain::message::Sum::BlockResponse(response) => {
                let Some(proto) = response.block else {
                    return false;
                };
                let Ok(block) = Block::try_from_proto(&proto) else {
                    return false;
                };
                if block.header.height < 0 {
                    return false;
                }
                inner.accept_block(peer_id, block)
            }
        };
        drop(inner);
        match outcome {
            Outcome::Ok => true,
            Outcome::Stop => false,
            Outcome::Reply(bytes) => {
                let _ = switch.send(peer_id, BLOCKCHAIN_CHANNEL, &bytes);
                true
            }
            Outcome::StopOther(other) => {
                if other != peer_id {
                    switch.stop_peer(&other);
                    true
                } else {
                    false
                }
            }
        }
    }
}

enum Outcome {
    Ok,
    Stop,
    Reply(Vec<u8>),
    /// A buffered block from this peer failed when it became the next height.
    StopOther(String),
}

impl<A: App, D: Db> Inner<A, D> {
    fn block_answer(&self, height: i64) -> blockchain::message::Sum {
        match self.blocks.load_block(height) {
            Some(block) => blockchain::message::Sum::BlockResponse(blockchain::BlockResponse {
                block: Some(block.to_proto()),
            }),
            None => {
                blockchain::message::Sum::NoBlockResponse(blockchain::NoBlockResponse { height })
            }
        }
    }

    fn accept_block(&mut self, peer_id: &str, block: Block) -> Outcome {
        let our_height = self.blocks.height();
        let Some(incoming) = self.pool.take_block(peer_id, our_height, block) else {
            return Outcome::Stop;
        };
        match incoming {
            Incoming::Later | Incoming::Duplicate => Outcome::Ok,
            Incoming::Next(block) => {
                let block = *block;
                if let Err(bad_peer) = self.apply_block(peer_id, block) {
                    return Outcome::StopOther(bad_peer);
                }
                match self.apply_ready().into_iter().next() {
                    Some(bad_peer) => Outcome::StopOther(bad_peer),
                    None => Outcome::Ok,
                }
            }
        }
    }

    /// Apply buffered blocks that are now the next height.
    ///
    /// Returns the peer ids whose blocks failed the commit or `ApplyBlock` check.
    fn apply_ready(&mut self) -> Vec<String> {
        let mut stopped = Vec::new();
        loop {
            let next = self.blocks.height() + 1;
            let Some(buffered) = self.pool.pop_next(next) else {
                break;
            };
            if let Err(bad_peer) = self.apply_block(&buffered.peer_id, buffered.block) {
                stopped.push(bad_peer);
                break;
            }
        }
        stopped
    }

    /// Check the commit, apply, then save the block and the state.
    ///
    /// The error is the peer that sent the block. Nothing is written on failure.
    fn apply_block(&mut self, peer_id: &str, block: Block) -> Result<(), String> {
        if !self.commit_matches(&block) {
            return Err(peer_id.to_owned());
        }
        if validate_block(&self.state, &block).is_err() {
            return Err(peer_id.to_owned());
        }
        let parts = block
            .make_part_set(BLOCK_PART_SIZE_BYTES)
            .map_err(|_| peer_id.to_owned())?;
        let hash = block
            .hash()
            .map(|hash| hash.as_bytes().to_vec())
            .ok_or_else(|| peer_id.to_owned())?;
        let block_id = BlockId {
            hash,
            part_set_header: parts.header(),
        };
        let next = apply_block(&self.state, &block_id, &block, &mut self.app)
            .map_err(|_| peer_id.to_owned())?;
        let seen = Commit {
            height: block.header.height,
            round: 0,
            block_id: block_id.clone(),
            signatures: vec![CommitSig::absent()],
        };
        self.blocks
            .save_block(&block, &parts, &seen)
            .map_err(|_| peer_id.to_owned())?;
        self.states.save(&next).map_err(|_| peer_id.to_owned())?;
        self.state = next;
        Ok(())
    }

    /// Height 1 keeps the empty commit `validate_block` allows.
    ///
    /// Above the initial height, `last_commit.block_id` must be the previous
    /// block's id. That commit is what proves the previous block.
    fn commit_matches(&self, block: &Block) -> bool {
        if block.header.height <= self.state.initial_height {
            return true;
        }
        let Some(commit) = &block.last_commit else {
            return false;
        };
        let Some(meta) = self.blocks.load_block_meta(block.header.height - 1) else {
            return false;
        };
        commit.block_id == meta.block_id
    }
}

fn status_request() -> blockchain::message::Sum {
    blockchain::message::Sum::StatusRequest(blockchain::StatusRequest {})
}

fn status_response(height: i64, base: i64) -> blockchain::message::Sum {
    blockchain::message::Sum::StatusResponse(blockchain::StatusResponse { height, base })
}

fn block_request(height: i64) -> blockchain::message::Sum {
    blockchain::message::Sum::BlockRequest(blockchain::BlockRequest { height })
}

fn encode(sum: blockchain::message::Sum) -> Vec<u8> {
    Message { sum: Some(sum) }.encode_to_vec()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
