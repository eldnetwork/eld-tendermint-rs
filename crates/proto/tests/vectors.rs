#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Hex vectors from `tests/vectors/`, copied from the commit in `proto/GO_REF`.
//!
//! Covered here:
//! - `mempool/v0/reactor_test.go` and `mempool/v1/reactor_test.go` `TestMempoolVectors`
//! - `blockchain/msgs_test.go` `TestBlockchainMessageVectors` (including `BlockResponse`)
//! - `privval/msgs_test.go` `TestPrivvalVectors` (ping, pubkey, vote, and proposal rows)
//! - `consensus/msgs_test.go` `TestConsMsgsVectors`
//! - `p2p/pex/pex_reactor_test.go` `TestPexVectors`
//! - `p2p/conn/connection_test.go` `TestConnVectors`
//! - `statesync/messages_test.go` `TestStateSyncVectors`
//! - `evidence/reactor_test.go` `TestEvidenceVectors`
//! - `types/results_test.go` `TestABCIResults` (nil versus empty `Data`)
//! - `abci/types/messages_test.go` `TestWriteReadMessageSimple` (`RequestEcho`)
//!
//! `types/vote_test.go` `TestVoteSignBytesTestVectors` lives in
//! `crates/types/tests/sign_bytes.rs` (canonical bytes, not a proto wrapper).
//! `types/protobuf_test.go` generates keys and has no static hex.
//! The secret-connection golden file is `tests/vectors/secret-connection.golden`.

use prost::Message;
use prost_types::Timestamp;

use eld_tendermint_crypto::{hash_from_byte_slices, sum, sum_truncated};
use eld_tendermint_proto::abci::{RequestEcho, ResponseDeliverTx};
use eld_tendermint_proto::blockchain::{
    BlockRequest, BlockResponse, Message as BcMessage, NoBlockResponse, StatusRequest,
    StatusResponse, message as bc_message,
};
use eld_tendermint_proto::consensus::{
    BlockPart, HasVote, Message as ConsMessage, NewRoundStep, NewValidBlock, ProposalPol,
    VoteSetBits, VoteSetMaj23, message as cons_message,
};
use eld_tendermint_proto::crypto::{Proof, PublicKey, public_key};
use eld_tendermint_proto::libs::bits::BitArray;
use eld_tendermint_proto::mempool::{Message as MemMessage, Txs, message as mem_message};
use eld_tendermint_proto::p2p::{
    Message as PexMessage, NetAddress, Packet, PacketMsg, PacketPing, PacketPong, PexAddrs,
    PexRequest, message as pex_message, packet as packet_sum,
};
use eld_tendermint_proto::privval::{
    Message as PvMessage, PingRequest, PingResponse, PubKeyRequest, PubKeyResponse,
    RemoteSignerError, SignProposalRequest, SignVoteRequest, SignedProposalResponse,
    SignedVoteResponse, message as pv_message,
};
use eld_tendermint_proto::statesync::{
    ChunkRequest, ChunkResponse, Message as SsMessage, SnapshotsRequest, SnapshotsResponse,
    message as ss_message,
};
use eld_tendermint_proto::types::{
    self, Block, BlockId, Data, DuplicateVoteEvidence, Evidence, EvidenceList, Header, Part,
    PartSetHeader, SignedMsgType,
};
use eld_tendermint_proto::version;

/// `time.Time{}` as a protobuf timestamp (year 1). gogoproto emits this for a zero `time.Time`.
fn go_zero_time() -> Timestamp {
    Timestamp {
        seconds: -62_135_596_800,
        nanos: 0,
    }
}

fn timestamp(seconds: i64, nanos: i32) -> Timestamp {
    Timestamp { seconds, nanos }
}

fn block_id(hash: Vec<u8>, total: u32, part_hash: Vec<u8>) -> BlockId {
    BlockId {
        hash,
        part_set_header: Some(PartSetHeader {
            total,
            hash: part_hash,
        }),
    }
}

/// Zero `BlockID`. `part_set_header` is `nullable = false`, so Go still emits the empty message.
fn empty_block_id() -> BlockId {
    block_id(Vec::new(), 0, Vec::new())
}

/// `bits.NewBitArray(1)`: one bit, still unset.
fn one_bit() -> BitArray {
    BitArray {
        bits: 1,
        elems: vec![0],
    }
}

/// Zero `BitArray`. Used where the proto field is `nullable = false` and the Go value is nil.
fn empty_bit_array() -> BitArray {
    BitArray {
        bits: 0,
        elems: Vec::new(),
    }
}

fn merkle_root(leaves: &[impl AsRef<[u8]>]) -> Vec<u8> {
    hash_from_byte_slices(leaves).to_vec()
}

fn vector_hex(text: &'static str) -> Vec<&'static str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect()
}

fn assert_hex<M>(msg: &M, expected: &str)
where
    M: Message + Default + PartialEq + std::fmt::Debug,
{
    let encoded = msg.encode_to_vec();
    let got = hex::encode(&encoded);
    assert_eq!(
        got, expected,
        "encode mismatch\n got: {got}\nwant: {expected}"
    );

    let decoded = M::decode(encoded.as_slice()).expect("decode encoded bytes");
    let decoded_hex = hex::encode(decoded.encode_to_vec());
    assert_eq!(
        &decoded, msg,
        "decode mismatch\n got: {decoded_hex}\nwant: {expected}"
    );

    let raw = hex::decode(expected).expect("expected hex");
    let from_hex = M::decode(raw.as_slice()).expect("decode expected hex");
    let from_hex_hex = hex::encode(from_hex.encode_to_vec());
    assert_eq!(
        &from_hex, msg,
        "decode-from-hex mismatch\n got: {from_hex_hex}\nwant: {expected}"
    );
}

/// `mempool/v0/reactor_test.go` and `mempool/v1/reactor_test.go` `TestMempoolVectors`.
#[test]
fn mempool_vectors() {
    let mut expected =
        vector_hex(include_str!("../../../tests/vectors/mempool-v0.hex")).into_iter();
    assert_eq!(
        vector_hex(include_str!("../../../tests/vectors/mempool-v0.hex")),
        vector_hex(include_str!("../../../tests/vectors/mempool-v1.hex")),
    );
    let cases = [
        (vec![0x7b], expected.next().expect("vector")),
        (
            b"proto encoding in mempool".to_vec(),
            expected.next().expect("vector"),
        ),
    ];
    for (tx, expected) in cases {
        let msg = MemMessage {
            sum: Some(mem_message::Sum::Txs(Txs { txs: vec![tx] })),
        };
        assert_hex(&msg, expected);
    }
}

/// `types.MakeBlock(3, []Tx{"Hello World"}, nil, nil)` with `Version.Block = 11`.
///
/// `LastCommit` is nil, so `last_commit` and `last_commit_hash` are omitted.
/// `Data.Hash` is the Merkle root of the tx hashes. Empty evidence hashes as an empty tree.
fn hello_world_block() -> Block {
    let tx_hash = sum(b"Hello World");
    Block {
        header: Some(Header {
            version: Some(version::Consensus { block: 11, app: 0 }),
            chain_id: String::new(),
            height: 3,
            time: Some(go_zero_time()),
            last_block_id: Some(empty_block_id()),
            last_commit_hash: Vec::new(),
            data_hash: merkle_root(&[&tx_hash[..]]),
            validators_hash: Vec::new(),
            next_validators_hash: Vec::new(),
            consensus_hash: Vec::new(),
            app_hash: Vec::new(),
            last_results_hash: Vec::new(),
            evidence_hash: merkle_root(&[] as &[&[u8]]),
            proposer_address: Vec::new(),
        }),
        data: Some(Data {
            txs: vec![b"Hello World".to_vec()],
        }),
        evidence: Some(EvidenceList {
            evidence: Vec::new(),
        }),
        last_commit: None,
    }
}

/// `blockchain/msgs_test.go` `TestBlockchainMessageVectors`.
#[test]
fn blockchain_message_vectors() {
    let mut expected =
        vector_hex(include_str!("../../../tests/vectors/blockchain.hex")).into_iter();
    let cases: Vec<(BcMessage, &str)> = vec![
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockRequest(BlockRequest { height: 1 })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockRequest(BlockRequest {
                    height: i64::MAX,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockResponse(BlockResponse {
                    block: Some(hello_world_block()),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::NoBlockResponse(NoBlockResponse {
                    height: 1,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::NoBlockResponse(NoBlockResponse {
                    height: i64::MAX,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusRequest(StatusRequest {})),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusResponse(StatusResponse {
                    height: 1,
                    base: 2,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusResponse(StatusResponse {
                    height: i64::MAX,
                    base: i64::MAX,
                })),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `privval/msgs_test.go` `exampleVote`: type 1, height 3, round 2, stamp 2019-10-13 16:14:44 UTC.
fn example_privval_vote() -> types::Vote {
    types::Vote {
        r#type: SignedMsgType::Prevote as i32,
        height: 3,
        round: 2,
        block_id: Some(block_id(
            sum(b"blockID_hash").to_vec(),
            1_000_000,
            sum(b"blockID_part_set_header_hash").to_vec(),
        )),
        timestamp: Some(timestamp(1_570_983_284, 0)),
        validator_address: sum_truncated(b"validator_address").to_vec(),
        validator_index: 56_789,
        signature: Vec::new(),
    }
}

/// `privval/msgs_test.go` `exampleProposal`. `Type` is `SignedMsgType(1)`, not `ProposalType`.
fn example_privval_proposal() -> types::Proposal {
    types::Proposal {
        r#type: SignedMsgType::Prevote as i32,
        height: 3,
        round: 2,
        pol_round: 2,
        block_id: Some(block_id(
            sum(b"blockID_hash").to_vec(),
            1_000_000,
            sum(b"blockID_part_set_header_hash").to_vec(),
        )),
        timestamp: Some(timestamp(1_570_983_284, 0)),
        signature: b"it's a signature".to_vec(),
    }
}

/// Zero vote. `block_id` and `timestamp` are `nullable = false`, so Go emits both.
fn empty_vote() -> types::Vote {
    types::Vote {
        r#type: SignedMsgType::Unknown as i32,
        height: 0,
        round: 0,
        block_id: Some(empty_block_id()),
        timestamp: Some(go_zero_time()),
        validator_address: Vec::new(),
        validator_index: 0,
        signature: Vec::new(),
    }
}

/// Zero proposal. Same non-nullable `block_id` and `timestamp` as [`empty_vote`].
fn empty_proposal() -> types::Proposal {
    types::Proposal {
        r#type: SignedMsgType::Unknown as i32,
        height: 0,
        round: 0,
        pol_round: 0,
        block_id: Some(empty_block_id()),
        timestamp: Some(go_zero_time()),
        signature: Vec::new(),
    }
}

/// `privval/msgs_test.go` `TestPrivvalVectors`.
#[test]
fn privval_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/privval.hex")).into_iter();
    let ed25519 = hex::decode("556a436f1218d30942efe798420f51dc9b6a311b929c578257457d05c5fcf230")
        .expect("pubkey hex");
    let remote_error = RemoteSignerError {
        code: 1,
        description: "it's a error".to_string(),
    };
    let vote = example_privval_vote();
    let proposal = example_privval_proposal();

    let cases: Vec<(PvMessage, &str)> = vec![
        (
            PvMessage {
                sum: Some(pv_message::Sum::PingRequest(PingRequest {})),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PingResponse(PingResponse {})),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PubKeyRequest(PubKeyRequest {
                    chain_id: String::new(),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PubKeyResponse(PubKeyResponse {
                    pub_key: Some(PublicKey {
                        sum: Some(public_key::Sum::Ed25519(ed25519)),
                    }),
                    error: None,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PubKeyResponse(PubKeyResponse {
                    // gogoproto.nullable = false: Go still emits the empty pubkey.
                    pub_key: Some(PublicKey { sum: None }),
                    error: Some(remote_error.clone()),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignVoteRequest(SignVoteRequest {
                    vote: Some(vote.clone()),
                    chain_id: String::new(),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedVoteResponse(SignedVoteResponse {
                    vote: Some(vote),
                    error: None,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedVoteResponse(SignedVoteResponse {
                    vote: Some(empty_vote()),
                    error: Some(remote_error.clone()),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignProposalRequest(SignProposalRequest {
                    proposal: Some(proposal.clone()),
                    chain_id: String::new(),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedProposalResponse(
                    SignedProposalResponse {
                        proposal: Some(proposal),
                        error: None,
                    },
                )),
            },
            expected.next().expect("vector"),
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedProposalResponse(
                    SignedProposalResponse {
                        proposal: Some(empty_proposal()),
                        error: Some(remote_error),
                    },
                )),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `consensus/msgs_test.go` `TestConsMsgsVectors`.
///
/// The block id hash is the raw 32-byte string, not a tmhash of it.
#[test]
fn cons_msgs_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/consensus.hex")).into_iter();
    let marks = b"add_more_exclamation_marks_code-".to_vec();
    let shorter = b"add_more_exclamation".to_vec();
    let psh = PartSetHeader {
        total: 1,
        hash: marks.clone(),
    };
    let block = BlockId {
        hash: marks.clone(),
        part_set_header: Some(psh.clone()),
    };
    let bits = one_bit();
    // 2018-08-30 12:00:00 UTC.
    let date = timestamp(1_535_630_400, 0);
    let part = Part {
        index: 1,
        bytes: b"test".to_vec(),
        proof: Some(Proof {
            total: 1,
            index: 1,
            leaf_hash: marks,
            aunts: Vec::new(),
        }),
    };
    let proposal = types::Proposal {
        r#type: SignedMsgType::Proposal as i32,
        height: 1,
        round: 1,
        pol_round: 1,
        block_id: Some(block.clone()),
        timestamp: Some(date),
        signature: shorter.clone(),
    };
    let vote = types::Vote {
        r#type: SignedMsgType::Precommit as i32,
        height: 1,
        round: 0,
        block_id: Some(block.clone()),
        timestamp: Some(date),
        validator_address: shorter,
        validator_index: 1,
        signature: Vec::new(),
    };

    let cases: Vec<(ConsMessage, &str)> = vec![
        (
            ConsMessage {
                sum: Some(cons_message::Sum::NewRoundStep(NewRoundStep {
                    height: 1,
                    round: 1,
                    step: 1,
                    seconds_since_start_time: 1,
                    last_commit_round: 1,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::NewRoundStep(NewRoundStep {
                    height: i64::MAX,
                    round: i32::MAX,
                    step: u32::MAX,
                    seconds_since_start_time: i64::MAX,
                    last_commit_round: i32::MAX,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::NewValidBlock(NewValidBlock {
                    height: 1,
                    round: 1,
                    block_part_set_header: Some(psh),
                    block_parts: Some(bits.clone()),
                    is_commit: false,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::Proposal(
                    eld_tendermint_proto::consensus::Proposal {
                        proposal: Some(proposal),
                    },
                )),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::ProposalPol(ProposalPol {
                    height: 1,
                    proposal_pol_round: 1,
                    // nullable = false: a nil Go bit array is still an empty message.
                    proposal_pol: Some(empty_bit_array()),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::BlockPart(BlockPart {
                    height: 1,
                    round: 1,
                    part: Some(part),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::Vote(
                    eld_tendermint_proto::consensus::Vote { vote: Some(vote) },
                )),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::HasVote(HasVote {
                    height: 1,
                    round: 1,
                    r#type: SignedMsgType::Prevote as i32,
                    index: 1,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::HasVote(HasVote {
                    height: i64::MAX,
                    round: i32::MAX,
                    r#type: SignedMsgType::Prevote as i32,
                    index: i32::MAX,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::VoteSetMaj23(VoteSetMaj23 {
                    height: 1,
                    round: 1,
                    r#type: SignedMsgType::Prevote as i32,
                    block_id: Some(block.clone()),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::VoteSetBits(VoteSetBits {
                    height: 1,
                    round: 1,
                    r#type: SignedMsgType::Prevote as i32,
                    block_id: Some(block),
                    votes: Some(bits),
                })),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `p2p/pex/pex_reactor_test.go` `TestPexVectors`. Wrapped in `p2p.Message`.
#[test]
fn pex_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/pex.hex")).into_iter();
    let addr = NetAddress {
        id: "1".to_string(),
        ip: "127.0.0.1".to_string(),
        port: 9090,
    };
    let cases: Vec<(PexMessage, &str)> = vec![
        (
            PexMessage {
                sum: Some(pex_message::Sum::PexRequest(PexRequest {})),
            },
            expected.next().expect("vector"),
        ),
        (
            PexMessage {
                sum: Some(pex_message::Sum::PexAddrs(PexAddrs { addrs: vec![addr] })),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `p2p/conn/connection_test.go` `TestConnVectors`. Wrapped in `p2p.Packet`.
#[test]
fn conn_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/conn.hex")).into_iter();
    let cases: Vec<(Packet, &str)> = vec![
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketPing(PacketPing {})),
            },
            expected.next().expect("vector"),
        ),
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketPong(PacketPong {})),
            },
            expected.next().expect("vector"),
        ),
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketMsg(PacketMsg {
                    channel_id: 1,
                    eof: false,
                    data: b"data transmitted over the wire".to_vec(),
                })),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `statesync/messages_test.go` `TestStateSyncVectors`. Wrapped in `statesync.Message`.
#[test]
fn state_sync_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/statesync.hex")).into_iter();
    let cases: Vec<(SsMessage, &str)> = vec![
        (
            SsMessage {
                sum: Some(ss_message::Sum::SnapshotsRequest(SnapshotsRequest {})),
            },
            expected.next().expect("vector"),
        ),
        (
            SsMessage {
                sum: Some(ss_message::Sum::SnapshotsResponse(SnapshotsResponse {
                    height: 1,
                    format: 2,
                    chunks: 3,
                    hash: b"chuck hash".to_vec(),
                    metadata: b"snapshot metadata".to_vec(),
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            SsMessage {
                sum: Some(ss_message::Sum::ChunkRequest(ChunkRequest {
                    height: 1,
                    format: 2,
                    index: 3,
                })),
            },
            expected.next().expect("vector"),
        ),
        (
            SsMessage {
                sum: Some(ss_message::Sum::ChunkResponse(ChunkResponse {
                    height: 1,
                    format: 2,
                    index: 3,
                    chunk: b"it's a chunk".to_vec(),
                    missing: false,
                })),
            },
            expected.next().expect("vector"),
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `evidence/reactor_test.go` `exampleVote`. Stamp is `2017-12-25T03:00:01.234Z`.
fn example_evidence_vote(vote_type: SignedMsgType) -> types::Vote {
    types::Vote {
        r#type: vote_type as i32,
        height: 3,
        round: 2,
        block_id: Some(block_id(
            sum(b"blockID_hash").to_vec(),
            1_000_000,
            sum(b"blockID_part_set_header_hash").to_vec(),
        )),
        timestamp: Some(timestamp(1_514_170_801, 234_000_000)),
        validator_address: sum_truncated(b"validator_address").to_vec(),
        validator_index: 56_789,
        signature: Vec::new(),
    }
}

/// `evidence/reactor_test.go` `TestEvidenceVectors`.
///
/// Both votes share a block id, so `NewDuplicateVoteEvidence` keeps vote B as the
/// prevote (`exampleVote(1)`) and vote A as the precommit (`exampleVote(2)`).
/// `defaultEvidenceTime` is 2019-01-01 UTC. One validator of power 10.
#[test]
fn evidence_vectors() {
    let mut expected = vector_hex(include_str!("../../../tests/vectors/evidence.hex")).into_iter();
    let list = EvidenceList {
        evidence: vec![Evidence {
            sum: Some(types::evidence::Sum::DuplicateVoteEvidence(
                DuplicateVoteEvidence {
                    vote_a: Some(example_evidence_vote(SignedMsgType::Precommit)),
                    vote_b: Some(example_evidence_vote(SignedMsgType::Prevote)),
                    total_voting_power: 10,
                    validator_power: 10,
                    timestamp: Some(timestamp(1_546_300_800, 0)),
                },
            )),
        }],
    };
    assert_hex(&list, expected.next().expect("vector"));
}

/// `types/results_test.go` `TestABCIResults` marshal checks.
///
/// Go asserts nil `Data` and empty `[]byte{}` encode to the same bytes. Prost has a single
/// empty `Bytes` value for both. Merkle proofs over these results belong with the types crate.
#[test]
fn abci_results_marshal() {
    let empty = deliver_tx(0, &[]);
    let one = deliver_tx(0, b"one");
    let code_only = deliver_tx(14, &[]);
    let foo = deliver_tx(14, b"foo");
    let bar = deliver_tx(14, b"bar");

    assert_eq!(empty.encode_to_vec(), Vec::<u8>::new());
    let mut last = empty.encode_to_vec();
    for result in [&one, &code_only, &foo, &bar] {
        let encoded = result.encode_to_vec();
        assert_ne!(last, encoded);
        last = encoded;
    }
}

/// `abci/types/messages_test.go` `TestWriteReadMessageSimple` for `RequestEcho`.
/// Length-prefixed socket framing stays in the later abci crate.
#[test]
fn request_echo_round_trip() {
    let msg = RequestEcho {
        message: "Hello".to_string(),
    };
    let decoded = RequestEcho::decode(msg.encode_to_vec().as_slice()).expect("decode");
    assert_eq!(decoded, msg);
}

fn deliver_tx(code: u32, data: &[u8]) -> ResponseDeliverTx {
    ResponseDeliverTx {
        code,
        data: bytes::Bytes::copy_from_slice(data),
        log: String::new(),
        info: String::new(),
        gas_wanted: 0,
        gas_used: 0,
        events: Vec::new(),
        codespace: String::new(),
    }
}
