//! Hex vectors copied from Eld Tendermint `v0.34.24-eld.3` tests.
//! Cases that need `MakeBlock`, ed25519, or canonical votes are left for later crates.

use prost::Message;

use eld_tendermint_proto::abci::{RequestEcho, ResponseDeliverTx};
use eld_tendermint_proto::blockchain::{
    BlockRequest, Message as BcMessage, NoBlockResponse, StatusRequest, StatusResponse,
    message as bc_message,
};
use eld_tendermint_proto::crypto::{PublicKey, public_key};
use eld_tendermint_proto::mempool::{Message as MemMessage, Txs, message as mem_message};
use eld_tendermint_proto::privval::{
    Message as PvMessage, PingRequest, PingResponse, PubKeyRequest, PubKeyResponse,
    RemoteSignerError, message as pv_message,
};

fn hex_encode(msg: &impl Message) -> String {
    hex::encode(msg.encode_to_vec())
}

fn assert_hex(msg: &impl Message, expected: &str) {
    assert_eq!(hex_encode(msg), expected);
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

/// `blockchain/msgs_test.go` `TestBlockchainMessageVectors`, except `BlockResponseMessage`.
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

/// `privval/msgs_test.go` `TestPrivvalVectors` rows that do not embed a vote or proposal.
#[test]
fn privval_vectors() {
    let ed25519 = hex::decode("556a436f1218d30942efe798420f51dc9b6a311b929c578257457d05c5fcf230")
        .expect("pubkey hex");
    let remote_error = RemoteSignerError {
        code: 1,
        description: "it's a error".to_string(),
    };

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
                    error: Some(remote_error),
                })),
            },
            "12140a0012100801120c697427732061206572726f72",
        ),
    ];
    for (msg, expected) in cases {
        assert_hex(&msg, expected);
    }
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
