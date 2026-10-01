//! Block cases from `types/block_test.go`.
//!
//! Cases that need duplicate-vote evidence or a random vote set are left for later.

use eld_tendermint_crypto::hash_from_byte_slices;
use eld_tendermint_proto::types::BlockIdFlag;
use eld_tendermint_types::{
    ADDRESS_SIZE, Block, BlockId, Commit, CommitSig, Error, EvidenceList, PartSetHeader, Time, Tx,
    Txs,
};
use prost::Message;

/// `Block` bytes inside `TestBlockchainMessageVectors` `BlockResponse`.
///
/// The Go hex is a blockchain `Message`: `1a70` wraps `BlockResponse`, and `0a6e`
/// wraps this `Block`. `MakeBlock(3, []Tx{"Hello World"}, nil, nil)` with block version 11.
const HELLO_WORLD_BLOCK_HEX: &str = "0a5b0a02080b1803220b088092b8c398feffffff012a0212003a20c4da88e876062aa1543400d50d0eaa0dac88096057949cfb7bca7f3a48c04bf96a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855120d0a0b48656c6c6f20576f726c641a00";

const EMPTY_ROOT: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const HELLO_WORLD_DATA_HASH: &str =
    "c4da88e876062aa1543400d50d0eaa0dac88096057949cfb7bca7f3a48c04bf9";

fn hello_world() -> Block {
    Block::make_block(
        3,
        Txs::new(vec![Tx::new(b"Hello World".to_vec())]),
        None,
        EvidenceList,
    )
}

fn valid_commit() -> Commit {
    Commit {
        height: 2,
        round: 1,
        block_id: BlockId {
            hash: vec![0xab; 32],
            part_set_header: PartSetHeader {
                total: 1,
                hash: vec![0xcd; 32],
            },
        },
        signatures: vec![CommitSig {
            block_id_flag: BlockIdFlag::Commit,
            validator_address: vec![0x11; ADDRESS_SIZE],
            timestamp: Time::GO_ZERO,
            signature: vec![1],
        }],
    }
}

fn valid_block(txs: Txs) -> Block {
    let mut block = Block::make_block(3, txs, Some(valid_commit()), EvidenceList);
    block.header.proposer_address = vec![0x22; ADDRESS_SIZE];
    block
}

#[test]
fn make_block_hashes_match_tx_merkle_roots() {
    let empty = Block::make_block(3, Txs::new(vec![]), None, EvidenceList);
    let one = hello_world();
    let many = Block::make_block(
        3,
        Txs::new(vec![Tx::new(b"foo".to_vec()), Tx::new(b"bar".to_vec())]),
        None,
        EvidenceList,
    );

    for block in [&empty, &one, &many] {
        assert_eq!(
            block.header.data_hash,
            block.data.hash().as_bytes().to_vec()
        );
        assert_eq!(
            block.header.evidence_hash,
            EvidenceList.hash().as_bytes().to_vec()
        );
        assert!(block.header.last_commit_hash.is_empty());
        assert_eq!(block.hash(), None);
        assert!(block.header.proposer_address.is_empty());
        assert!(block.header.last_block_id.is_zero());
        assert!(block.header.time.is_zero());
    }

    assert_eq!(hex::encode(&empty.header.data_hash), EMPTY_ROOT);
    assert_eq!(hex::encode(&one.header.data_hash), HELLO_WORLD_DATA_HASH);
    assert_eq!(hex::encode(&empty.header.evidence_hash), EMPTY_ROOT);
    assert_ne!(many.header.data_hash, one.header.data_hash);
    assert_ne!(many.header.data_hash, empty.header.data_hash);
}

#[test]
fn hello_world_protobuf_matches_go_block_response() {
    let encoded = hello_world().to_proto().encode_to_vec();
    let expected = hex::decode(HELLO_WORLD_BLOCK_HEX).expect("block hex");
    assert_eq!(
        hex::encode(&encoded),
        hex::encode(&expected),
        "encode mismatch\n got: {}\nwant: {}",
        hex::encode(&encoded),
        hex::encode(&expected)
    );
}

#[test]
fn make_part_set_round_trips_block_bytes() {
    let block = hello_world();
    let encoded = block.to_proto().encode_to_vec();
    let part_set = block.make_part_set(1024).expect("part set");
    assert_eq!(part_set.total(), 1);
    assert_eq!(part_set.read_bytes().expect("bytes"), encoded);
    let root = hash_from_byte_slices(&[&encoded[..]]);
    assert_eq!(part_set.hash(), root.as_slice());
}

#[test]
fn validate_basic_rejects_mutated_data_hash() {
    let mut block = valid_block(Txs::new(vec![
        Tx::new(b"foo".to_vec()),
        Tx::new(b"bar".to_vec()),
    ]));
    block.validate_basic().expect("valid block");

    block.header.data_hash = vec![0xff; 32];
    assert_eq!(block.validate_basic(), Err(Error::WrongDataHash));
}

#[test]
fn validate_basic_rejects_negative_height_and_nil_last_commit() {
    let negative = Block::make_block(1, Txs::new(vec![]), None, EvidenceList);
    let mut negative = negative;
    negative.header.height = -1;
    negative.header.proposer_address = vec![0x22; ADDRESS_SIZE];
    assert_eq!(negative.validate_basic(), Err(Error::NegativeHeight));

    let mut nil_commit = hello_world();
    nil_commit.header.proposer_address = vec![0x22; ADDRESS_SIZE];
    assert_eq!(nil_commit.validate_basic(), Err(Error::NilLastCommit));
}
