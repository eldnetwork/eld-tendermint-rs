#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Hex vectors copied from Eld Tendermint `v0.34.24-eld.3` tests.
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
//! `p2p/conn/secret_connection_test.go` (`TestSecretConnectionHandshake`,
//! `TestDeriveSecretsAndChallengeGolden`) is a handshake and KDF, not a proto
//! message. Those stay with the later p2p crate.

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
    let cases = [
        (vec![0x7b], "0a030a017b"),
        (
            b"proto encoding in mempool".to_vec(),
            "0a1b0a1970726f746f20656e636f64696e6720696e206d656d706f6f6c",
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
    let cases: Vec<(BcMessage, &str)> = vec![
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockRequest(BlockRequest { height: 1 })),
            },
            "0a020801",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockRequest(BlockRequest {
                    height: i64::MAX,
                })),
            },
            "0a0a08ffffffffffffffff7f",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::BlockResponse(BlockResponse {
                    block: Some(hello_world_block()),
                })),
            },
            "1a700a6e0a5b0a02080b1803220b088092b8c398feffffff012a0212003a20c4da88e876062aa1543400d50d0eaa0dac88096057949cfb7bca7f3a48c04bf96a20e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855120d0a0b48656c6c6f20576f726c641a00",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::NoBlockResponse(NoBlockResponse {
                    height: 1,
                })),
            },
            "12020801",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::NoBlockResponse(NoBlockResponse {
                    height: i64::MAX,
                })),
            },
            "120a08ffffffffffffffff7f",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusRequest(StatusRequest {})),
            },
            "2200",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusResponse(StatusResponse {
                    height: 1,
                    base: 2,
                })),
            },
            "2a0408011002",
        ),
        (
            BcMessage {
                sum: Some(bc_message::Sum::StatusResponse(StatusResponse {
                    height: i64::MAX,
                    base: i64::MAX,
                })),
            },
            "2a1408ffffffffffffffff7f10ffffffffffffffff7f",
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
            "3a00",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PingResponse(PingResponse {})),
            },
            "4200",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PubKeyRequest(PubKeyRequest {
                    chain_id: String::new(),
                })),
            },
            "0a00",
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
            "12240a220a20556a436f1218d30942efe798420f51dc9b6a311b929c578257457d05c5fcf230",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::PubKeyResponse(PubKeyResponse {
                    // gogoproto.nullable = false: Go still emits the empty pubkey.
                    pub_key: Some(PublicKey { sum: None }),
                    error: Some(remote_error.clone()),
                })),
            },
            "12140a0012100801120c697427732061206572726f72",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignVoteRequest(SignVoteRequest {
                    vote: Some(vote.clone()),
                    chain_id: String::new(),
                })),
            },
            "1a760a74080110031802224a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a2a0608f49a8ded0532146af1f4111082efb388211bc72c55bcd61e9ac3d538d5bb03",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedVoteResponse(SignedVoteResponse {
                    vote: Some(vote),
                    error: None,
                })),
            },
            "22760a74080110031802224a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a2a0608f49a8ded0532146af1f4111082efb388211bc72c55bcd61e9ac3d538d5bb03",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignedVoteResponse(SignedVoteResponse {
                    vote: Some(empty_vote()),
                    error: Some(remote_error.clone()),
                })),
            },
            "22250a11220212002a0b088092b8c398feffffff0112100801120c697427732061206572726f72",
        ),
        (
            PvMessage {
                sum: Some(pv_message::Sum::SignProposalRequest(SignProposalRequest {
                    proposal: Some(proposal.clone()),
                    chain_id: String::new(),
                })),
            },
            "2a700a6e08011003180220022a4a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a320608f49a8ded053a10697427732061207369676e6174757265",
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
            "32700a6e08011003180220022a4a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a320608f49a8ded053a10697427732061207369676e6174757265",
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
            "32250a112a021200320b088092b8c398feffffff0112100801120c697427732061206572726f72",
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
            "0a0a08011001180120012801",
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
            "0a2608ffffffffffffffff7f10ffffffff0718ffffffff0f20ffffffffffffffff7f28ffffffff07",
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
            "1231080110011a24080112206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d22050801120100",
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::Proposal(
                    eld_tendermint_proto::consensus::Proposal {
                        proposal: Some(proposal),
                    },
                )),
            },
            "1a720a7008201001180120012a480a206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d1224080112206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d320608c0b89fdc053a146164645f6d6f72655f6578636c616d6174696f6e",
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
            "2206080110011a00",
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::BlockPart(BlockPart {
                    height: 1,
                    round: 1,
                    part: Some(part),
                })),
            },
            "2a36080110011a3008011204746573741a26080110011a206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d",
        ),
        (
            ConsMessage {
                sum: Some(cons_message::Sum::Vote(
                    eld_tendermint_proto::consensus::Vote { vote: Some(vote) },
                )),
            },
            "32700a6e0802100122480a206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d1224080112206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d2a0608c0b89fdc0532146164645f6d6f72655f6578636c616d6174696f6e3801",
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
            "3a080801100118012001",
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
            "3a1808ffffffffffffffff7f10ffffffff07180120ffffffff07",
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
            "425008011001180122480a206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d1224080112206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d",
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
            "4a5708011001180122480a206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d1224080112206164645f6d6f72655f6578636c616d6174696f6e5f6d61726b735f636f64652d2a050801120100",
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `p2p/pex/pex_reactor_test.go` `TestPexVectors`. Wrapped in `p2p.Message`.
#[test]
fn pex_vectors() {
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
            "0a00",
        ),
        (
            PexMessage {
                sum: Some(pex_message::Sum::PexAddrs(PexAddrs { addrs: vec![addr] })),
            },
            "12130a110a013112093132372e302e302e31188247",
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `p2p/conn/connection_test.go` `TestConnVectors`. Wrapped in `p2p.Packet`.
#[test]
fn conn_vectors() {
    let cases: Vec<(Packet, &str)> = vec![
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketPing(PacketPing {})),
            },
            "0a00",
        ),
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketPong(PacketPong {})),
            },
            "1200",
        ),
        (
            Packet {
                sum: Some(packet_sum::Sum::PacketMsg(PacketMsg {
                    channel_id: 1,
                    eof: false,
                    data: b"data transmitted over the wire".to_vec(),
                })),
            },
            "1a2208011a1e64617461207472616e736d6974746564206f766572207468652077697265",
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
}

/// `statesync/messages_test.go` `TestStateSyncVectors`. Wrapped in `statesync.Message`.
#[test]
fn state_sync_vectors() {
    let cases: Vec<(SsMessage, &str)> = vec![
        (
            SsMessage {
                sum: Some(ss_message::Sum::SnapshotsRequest(SnapshotsRequest {})),
            },
            "0a00",
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
            "1225080110021803220a636875636b20686173682a11736e617073686f74206d65746164617461",
        ),
        (
            SsMessage {
                sum: Some(ss_message::Sum::ChunkRequest(ChunkRequest {
                    height: 1,
                    format: 2,
                    index: 3,
                })),
            },
            "1a06080110021803",
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
            "2214080110021803220c697427732061206368756e6b",
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
    assert_hex(
        &list,
        "0a85020a82020a79080210031802224a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a2a0b08b1d381d20510809dca6f32146af1f4111082efb388211bc72c55bcd61e9ac3d538d5bb031279080110031802224a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a2a0b08b1d381d20510809dca6f32146af1f4111082efb388211bc72c55bcd61e9ac3d538d5bb03180a200a2a060880dbaae105",
    );
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
