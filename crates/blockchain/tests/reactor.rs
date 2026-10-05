//! Two switches. A seeded chain, an empty peer, and a bad commit.

use std::os::unix::net::UnixStream;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use eld_tendermint_blockchain::{Reactor, channel_descriptors};
use eld_tendermint_crypto::PrivKey;
use eld_tendermint_p2p::{Switch, make_secret_connection};
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock, ResponseCommit,
    ResponseDeliverTx, ResponseEndBlock,
};
use eld_tendermint_proto::blockchain::{self, Message};
use eld_tendermint_proto::types::BlockIdFlag;
use eld_tendermint_state::{App, State, StateStore, apply_block, make_genesis_state};
use eld_tendermint_store::{BlockStore, MemDb};
use eld_tendermint_types::{
    BLOCK_PART_SIZE_BYTES, Block, BlockId, ChainId, Commit, CommitSig, EvidenceList, GenesisDoc,
    GenesisValidator, PartSetHeader, Time, Txs, hash_consensus_params, median_time,
};
use prost::Message as ProstMessage;
use prost::bytes::Bytes;

const APP_HASH: [u8; 32] = [0xab; 32];

struct Exec;

impl App for Exec {
    fn begin_block(
        &mut self,
        _: RequestBeginBlock,
    ) -> Result<ResponseBeginBlock, eld_tendermint_state::Error> {
        Ok(ResponseBeginBlock { events: Vec::new() })
    }

    fn deliver_tx(
        &mut self,
        _: RequestDeliverTx,
    ) -> Result<ResponseDeliverTx, eld_tendermint_state::Error> {
        Ok(ResponseDeliverTx {
            code: 0,
            data: Bytes::new(),
            log: String::new(),
            info: String::new(),
            gas_wanted: 0,
            gas_used: 0,
            events: Vec::new(),
            codespace: String::new(),
        })
    }

    fn end_block(
        &mut self,
        _: RequestEndBlock,
    ) -> Result<ResponseEndBlock, eld_tendermint_state::Error> {
        Ok(ResponseEndBlock {
            validator_updates: Vec::new(),
            consensus_param_updates: None,
            events: Vec::new(),
        })
    }

    fn commit(&mut self) -> Result<ResponseCommit, eld_tendermint_state::Error> {
        Ok(ResponseCommit {
            data: Bytes::copy_from_slice(&APP_HASH),
            retain_height: 0,
        })
    }
}

struct Chain {
    blocks: Arc<BlockStore<MemDb>>,
    states: Arc<StateStore<MemDb>>,
    state: State,
}

impl Chain {
    fn new(state: State) -> Self {
        Self {
            blocks: Arc::new(BlockStore::new(MemDb::new())),
            states: Arc::new(StateStore::new(MemDb::new())),
            state,
        }
    }

    fn extend(&mut self, count: i64) {
        let mut app = Exec;
        for _ in 0..count {
            let block = next_block(&self.state, &self.blocks);
            let proto = block.to_proto();
            let block = Block::try_from_proto(&proto).expect("seed block");
            let parts = block.make_part_set(BLOCK_PART_SIZE_BYTES).expect("parts");
            let block_id = BlockId {
                hash: block.hash().expect("hash").as_bytes().to_vec(),
                part_set_header: parts.header(),
            };
            let seen = Commit {
                height: block.header.height,
                round: 0,
                block_id: block_id.clone(),
                signatures: vec![CommitSig::absent()],
            };
            self.blocks
                .save_block(&block, &parts, &seen)
                .expect("save block");
            self.state = apply_block(&self.state, &block_id, &block, &mut app)
                .expect("apply")
                .state;
            self.states.save(&self.state).expect("save state");
        }
    }

    fn reactor(&self) -> Reactor<Exec, MemDb> {
        Reactor::new(
            Arc::clone(&self.blocks),
            Arc::clone(&self.states),
            self.state.clone(),
            Exec,
            None,
            None,
        )
    }
}

fn genesis_state() -> State {
    let pub_key = PrivKey::generate()
        .public_key()
        .expect("ed25519 public key");
    let mut doc = GenesisDoc {
        genesis_time: Time::from_unix_parts(1_600_000_000, 0),
        chain_id: ChainId::new("test-chain"),
        initial_height: 0,
        consensus_params: None,
        validators: vec![GenesisValidator {
            address: Vec::new(),
            pub_key,
            power: 10,
            name: String::new(),
        }],
        app_hash: vec![0x11; 32],
        app_state: None,
    };
    make_genesis_state(&mut doc).expect("genesis")
}

fn fill_header(block: &mut Block, state: &State) {
    block.header.chain_id = state.chain_id.clone();
    block.header.time = if block.header.height == state.initial_height {
        state.last_block_time
    } else if let Some(commit) = &block.last_commit {
        median_time(commit, &state.last_validators)
    } else {
        state.last_block_time
    };
    block.header.last_block_id = state.last_block_id.clone();
    block.header.validators_hash = state
        .validators
        .hash()
        .expect("validators hash")
        .as_bytes()
        .to_vec();
    block.header.next_validators_hash = state
        .next_validators
        .hash()
        .expect("next validators hash")
        .as_bytes()
        .to_vec();
    block.header.consensus_hash = hash_consensus_params(&state.consensus_params)
        .as_bytes()
        .to_vec();
    block.header.app_hash = state.app_hash.clone();
    block.header.last_results_hash = state.last_results_hash.clone();
    block.header.proposer_address = state
        .validators
        .proposer()
        .expect("proposer")
        .address
        .clone();
}

fn next_block(state: &State, blocks: &BlockStore<MemDb>) -> Block {
    let height = state.last_block_height + 1;
    let commit = if height == state.initial_height {
        Commit {
            height: 0,
            round: 0,
            block_id: BlockId::default(),
            signatures: Vec::new(),
        }
    } else {
        let prev = blocks
            .load_block_meta(state.last_block_height)
            .expect("previous block");
        Commit {
            height: state.last_block_height,
            round: 0,
            block_id: prev.block_id,
            signatures: vec![CommitSig {
                block_id_flag: BlockIdFlag::Commit,
                validator_address: state
                    .last_validators
                    .validators()
                    .first()
                    .map(|validator| validator.address.clone())
                    .unwrap_or_default(),
                timestamp: state
                    .last_block_time
                    .add_millis(state.consensus_params.block.time_iota_ms),
                signature: vec![0; 64],
            }],
        }
    };
    let mut block = Block::make_block(
        height,
        Txs::new(Vec::new()),
        Some(commit),
        EvidenceList::new(Vec::new()),
    );
    fill_header(&mut block, state);
    block
}

fn secret_pair() -> (
    eld_tendermint_p2p::SecretConnection<UnixStream>,
    eld_tendermint_p2p::SecretConnection<UnixStream>,
) {
    let key_a = PrivKey::generate();
    let key_b = PrivKey::generate();
    let (sock_a, sock_b) = UnixStream::pair().expect("socket pair");
    sock_a
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    sock_b
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let handle = thread::spawn(move || make_secret_connection(sock_b, &key_b).expect("handshake"));
    let conn_a = make_secret_connection(sock_a, &key_a).expect("handshake");
    let conn_b = handle.join().expect("peer handshake");
    (conn_a, conn_b)
}

fn switch_for(reactor: &Reactor<Exec, MemDb>) -> Arc<Switch> {
    let switch = Arc::new(Switch::new());
    let callback_switch = Arc::clone(&switch);
    let callback_reactor = reactor.clone();
    switch
        .add_reactor(
            "blockchain",
            channel_descriptors(),
            move |peer_id, ch_id, bytes| {
                if !callback_reactor.handle(&callback_switch, peer_id, ch_id, &bytes) {
                    callback_switch.stop_peer(peer_id);
                }
            },
        )
        .expect("reactor");
    switch
}

fn link(left: &Switch, right: &Switch) {
    let (conn_left, conn_right) = secret_pair();
    left.add_peer(conn_left, "right").expect("peer");
    right.add_peer(conn_right, "left").expect("peer");
}

fn encode_status(height: i64, base: i64) -> Vec<u8> {
    Message {
        sum: Some(blockchain::message::Sum::StatusResponse(
            blockchain::StatusResponse { height, base },
        )),
    }
    .encode_to_vec()
}

fn encode_block(block: &Block) -> Vec<u8> {
    Message {
        sum: Some(blockchain::message::Sum::BlockResponse(
            blockchain::BlockResponse {
                block: Some(block.to_proto()),
            },
        )),
    }
    .encode_to_vec()
}

#[test]
fn empty_peer_catches_up_to_height_three() {
    let genesis = genesis_state();
    let mut seeded = Chain::new(genesis.clone());
    seeded.extend(3);
    let behind = Chain::new(genesis);
    let ahead = seeded.reactor();
    let syncer = behind.reactor();
    let ahead_switch = switch_for(&ahead);
    let syncer_switch = switch_for(&syncer);
    link(&ahead_switch, &syncer_switch);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if syncer.height() == 3 && syncer.app_hash() == ahead.app_hash() {
            return;
        }
        ahead.poll(&ahead_switch);
        syncer.poll(&syncer_switch);
        thread::sleep(Duration::from_millis(2));
    }
    panic!("height {} app {}", syncer.height(), syncer.app_hash().len());
}

#[test]
fn bad_commit_stops_the_sender() {
    let mut chain = Chain::new(genesis_state());
    chain.extend(1);
    let reactor = chain.reactor();
    let receiver = switch_for(&reactor);
    let sender = Switch::new();
    sender
        .add_reactor("blockchain", channel_descriptors(), |_, _, _| {})
        .expect("sender reactor");
    link(&receiver, &sender);

    assert!(sender.send(
        "left",
        eld_tendermint_blockchain::BLOCKCHAIN_CHANNEL,
        &encode_status(2, 1)
    ));
    let start = Instant::now();
    while reactor.peer_height("right") != Some(2) && start.elapsed() < Duration::from_secs(2) {
        reactor.poll(&receiver);
        thread::sleep(Duration::from_millis(5));
    }
    reactor.poll(&receiver);
    assert_eq!(reactor.peer_height("right"), Some(2));
    assert_eq!(reactor.block_requests_sent(), 1);

    let mut bad = next_block(&chain.state, &chain.blocks);
    let previous = chain.blocks.load_block_meta(1).expect("height 1").block_id;
    bad.last_commit.as_mut().expect("commit").block_id = BlockId {
        hash: vec![0x11; 32],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![0x22; 32],
        },
    };
    assert_ne!(bad.last_commit.as_ref().expect("commit").block_id, previous);
    Block::try_from_proto(&bad.to_proto()).expect("bad block still decodes");
    thread::sleep(Duration::from_millis(30));
    assert!(sender.send(
        "left",
        eld_tendermint_blockchain::BLOCKCHAIN_CHANNEL,
        &encode_block(&bad)
    ));
    thread::sleep(Duration::from_millis(30));

    assert_eq!(reactor.height(), 1);
    assert!(receiver.peers().is_empty());
}

#[test]
fn equal_height_sends_no_block_request() {
    let genesis = genesis_state();
    let left = Chain::new(genesis.clone()).reactor();
    let right = Chain::new(genesis).reactor();
    let left_switch = switch_for(&left);
    let right_switch = switch_for(&right);
    link(&left_switch, &right_switch);

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        left.poll(&left_switch);
        right.poll(&right_switch);
        if left.peer_height("right") == Some(0) && right.peer_height("left") == Some(0) {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(left.peer_height("right"), Some(0));
    assert_eq!(right.peer_height("left"), Some(0));
    assert_eq!(left.block_requests_sent(), 0);
    assert_eq!(right.block_requests_sent(), 0);
}
