//! `ApplyBlock` against an in-process app. No socket.

use std::sync::Arc;

use eld_tendermint_crypto::{PrivKey, PubKey, pub_key_to_proto};
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestDeliverTx, RequestEndBlock, ResponseBeginBlock, ResponseCommit,
    ResponseDeliverTx, ResponseEndBlock, ValidatorUpdate,
};
use eld_tendermint_store::MemDb;
use eld_tendermint_types::{
    Block, BlockId, ChainId, Commit, CommitSig, EvidenceList, GenesisDoc, GenesisValidator, Time,
    Txs, ValidatorSet, hash_consensus_params,
};
use prost::bytes::Bytes;

use eld_tendermint_state::{App, Error, State, StateStore, apply_block, make_genesis_state};

const APP_HASH: [u8; 32] = [0xab; 32];

struct FakeApp {
    begin_count: u32,
    /// Returned from the next `end_block`, then cleared.
    update: Option<ValidatorUpdate>,
}

impl FakeApp {
    fn new(update: Option<ValidatorUpdate>) -> Self {
        Self {
            begin_count: 0,
            update,
        }
    }
}

impl App for FakeApp {
    fn begin_block(&mut self, _request: RequestBeginBlock) -> Result<ResponseBeginBlock, Error> {
        self.begin_count += 1;
        Ok(ResponseBeginBlock { events: Vec::new() })
    }

    fn deliver_tx(&mut self, _request: RequestDeliverTx) -> Result<ResponseDeliverTx, Error> {
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

    fn end_block(&mut self, _request: RequestEndBlock) -> Result<ResponseEndBlock, Error> {
        let update = self.update.take();
        Ok(ResponseEndBlock {
            validator_updates: update.into_iter().collect(),
            consensus_param_updates: None,
            events: Vec::new(),
        })
    }

    fn commit(&mut self) -> Result<ResponseCommit, Error> {
        Ok(ResponseCommit {
            data: Bytes::copy_from_slice(&APP_HASH),
            retain_height: 0,
        })
    }
}

fn key() -> PubKey {
    PrivKey::generate()
        .public_key()
        .expect("ed25519 public key")
}

fn genesis(pub_key: PubKey) -> GenesisDoc {
    GenesisDoc {
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
    }
}

fn fill_header(block: &mut Block, state: &State) {
    block.header.chain_id = state.chain_id.clone();
    block.header.time = state.last_block_time;
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

fn block_id(block: &Block) -> BlockId {
    BlockId {
        hash: block
            .header
            .hash()
            .expect("header hash")
            .as_bytes()
            .to_vec(),
        part_set_header: eld_tendermint_types::PartSetHeader::default(),
    }
}

fn height_one(state: &State) -> (Block, BlockId) {
    let mut block = Block::make_block(
        state.initial_height,
        Txs::new(Vec::new()),
        None,
        EvidenceList::new(Vec::new()),
    );
    fill_header(&mut block, state);
    let id = block_id(&block);
    (block, id)
}

fn contains(set: &ValidatorSet, address: &[u8]) -> bool {
    set.validators()
        .iter()
        .any(|validator| validator.address == address)
}

#[test]
fn genesis_height_one_commits_and_reloads() {
    let mut doc = genesis(key());
    let state = make_genesis_state(&mut doc).expect("genesis");
    assert_eq!(state.last_block_height, 0);
    let (block, id) = height_one(&state);
    assert!(block.last_commit.is_none());
    let mut app = FakeApp::new(None);
    let next = apply_block(&state, &id, &block, &mut app)
        .expect("apply height 1")
        .state;
    assert_eq!(next.app_hash, APP_HASH);
    let db = Arc::new(MemDb::new());
    StateStore::new(Arc::clone(&db)).save(&next).expect("save");
    let loaded = StateStore::new(db).load().expect("load");
    if loaded != next {
        panic!("reloaded state\n{loaded:?}\n!= applied state\n{next:?}");
    }
}

#[test]
fn wrong_validators_hash_does_not_begin_block() {
    let mut doc = genesis(key());
    let state = make_genesis_state(&mut doc).expect("genesis");
    let (mut block, id) = height_one(&state);
    block.header.validators_hash = vec![0; 32];
    let mut app = FakeApp::new(None);
    let err = apply_block(&state, &id, &block, &mut app).expect_err("bad validators hash");
    assert!(matches!(err, Error::WrongValidatorsHash), "{err}");
    assert_eq!(app.begin_count, 0);
}

#[test]
fn validator_update_lands_one_block_later() {
    let pub_key = key();
    let added = key();
    let mut doc = genesis(pub_key);
    let state = make_genesis_state(&mut doc).expect("genesis");
    let update = ValidatorUpdate {
        pub_key: Some(pub_key_to_proto(&added)),
        power: 20,
    };
    let (block, id) = height_one(&state);
    let mut app = FakeApp::new(Some(update));
    let after_one = apply_block(&state, &id, &block, &mut app)
        .expect("apply height 1")
        .state;
    let added_address = added.address();
    assert!(
        !contains(&after_one.validators, added_address.as_slice()),
        "validators already contain the update"
    );
    assert!(
        contains(&after_one.next_validators, added_address.as_slice()),
        "next_validators missing the update"
    );

    let commit = Commit {
        height: after_one.last_block_height,
        round: 0,
        block_id: id.clone(),
        signatures: vec![CommitSig::absent()],
    };
    let mut block = Block::make_block(
        after_one.last_block_height + 1,
        Txs::new(Vec::new()),
        Some(commit),
        EvidenceList::new(Vec::new()),
    );
    fill_header(&mut block, &after_one);
    let id = block_id(&block);
    let after_two = apply_block(&after_one, &id, &block, &mut app)
        .expect("apply height 2")
        .state;
    assert!(
        contains(&after_two.validators, added_address.as_slice()),
        "validators missing the update after the next block"
    );
}

#[test]
fn nil_last_commit_after_initial_height_is_rejected() {
    let mut doc = genesis(key());
    let state = make_genesis_state(&mut doc).expect("genesis");
    let (block, id) = height_one(&state);
    let mut app = FakeApp::new(None);
    let after_one = apply_block(&state, &id, &block, &mut app)
        .expect("apply height 1")
        .state;
    let mut block = Block::make_block(2, Txs::new(Vec::new()), None, EvidenceList::new(Vec::new()));
    fill_header(&mut block, &after_one);
    let id = block_id(&block);
    let err = apply_block(&after_one, &id, &block, &mut app).expect_err("nil commit at height 2");
    assert!(matches!(err, Error::NilLastCommit), "{err}");
}
