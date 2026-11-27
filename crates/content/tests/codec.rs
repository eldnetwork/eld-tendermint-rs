#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Encode and decode the content channel messages.

use eld_tendermint_content::{
    CONTENT_CHANNEL, MAX_BLOB_BYTES, RECV_MESSAGE_CAPACITY, blob_fits, channel_descriptors,
    message_fits,
};
use eld_tendermint_proto::content::{
    self, Announce, CapacityChallenge, CapacityChallengeResponse, ChunkProof, ContentPush,
    ContentPushAck, ContentSlot, Message, SlotState,
};
use prost::Message as ProstMessage;

const CONTENT_ID: &str = "abababababababababababababababababababababababababababababababab";

fn round_trip(message: &Message) -> Message {
    let bytes = message.encode_to_vec();
    assert!(
        message_fits(&bytes),
        "encoded {} bytes, cap is {RECV_MESSAGE_CAPACITY}",
        bytes.len()
    );
    Message::decode(bytes.as_slice()).unwrap()
}

#[test]
fn channel_is_0x50_and_holds_one_blob() {
    let desc = &channel_descriptors()[0];
    assert_eq!(desc.id, CONTENT_CHANNEL);
    assert_eq!(CONTENT_CHANNEL, 0x50);
    assert_eq!(desc.priority, 5);
    assert_eq!(desc.send_queue_capacity, 100);
    assert_eq!(desc.recv_message_capacity, RECV_MESSAGE_CAPACITY);
}

#[test]
fn push_and_receipt_round_trip() {
    let push = Message {
        sum: Some(content::message::Sum::ContentPush(ContentPush {
            request_id: 7,
            content_id: CONTENT_ID.to_owned(),
            blob: b"hello".to_vec(),
        })),
    };
    assert_eq!(round_trip(&push), push);

    let ack = Message {
        sum: Some(content::message::Sum::ContentPushAck(ContentPushAck {
            request_id: 7,
            content_id: CONTENT_ID.to_owned(),
            provider: vec![0x22; 20],
        })),
    };
    assert_eq!(round_trip(&ack), ack);

    let announce = Message {
        sum: Some(content::message::Sum::Announce(Announce {
            content_id: CONTENT_ID.to_owned(),
        })),
    };
    assert_eq!(round_trip(&announce), announce);
}

#[test]
fn challenge_response_keeps_the_proof_fields() {
    let challenge = Message {
        sum: Some(content::message::Sum::CapacityChallenge(
            CapacityChallenge {
                challenge_id: "challenge-1".to_owned(),
                challenger: vec![0x11; 20],
                provider: vec![0x22; 20],
                chunk_indices: vec![0, 3, 7],
                block_height: 42,
                merkle_root: vec![1; 32],
                seed: vec![2; 32],
                expiration_block: 142,
                timestamp: 1_700_000_000,
                node_id: vec![0x33; 20],
            },
        )),
    };
    assert_eq!(round_trip(&challenge), challenge);

    let response = Message {
        sum: Some(content::message::Sum::CapacityChallengeResponse(
            CapacityChallengeResponse {
                challenge_id: "challenge-1".to_owned(),
                provider: vec![0x22; 20],
                challenger: vec![0x11; 20],
                block_height: 42,
                proofs: vec![ChunkProof {
                    chunk_index: 3,
                    chunk_data: vec![9, 8, 7],
                    chunk_hash: vec![4; 32],
                    merkle_proof: vec![vec![5; 32], vec![6; 32]],
                    slot_state: Some(SlotState {
                        kind: Some(content::slot_state::Kind::Content(ContentSlot {
                            deal_id: "deal-1".to_owned(),
                            committed_hash: vec![7; 32],
                        })),
                    }),
                }],
                generated_at: 1_700_000_001,
                provider_pubkey: vec![8; 32],
                provider_signature: vec![9; 64],
                node_id: vec![0x33; 20],
            },
        )),
    };
    assert_eq!(round_trip(&response), response);
}

#[test]
fn a_blob_over_one_mib_does_not_fit() {
    assert!(blob_fits(MAX_BLOB_BYTES));
    assert!(!blob_fits(MAX_BLOB_BYTES + 1));

    let fitting = Message {
        sum: Some(content::message::Sum::ContentPush(ContentPush {
            request_id: 1,
            content_id: CONTENT_ID.to_owned(),
            blob: vec![0; MAX_BLOB_BYTES],
        })),
    };
    let encoded = fitting.encode_to_vec();
    assert!(message_fits(&encoded));
    assert!(!message_fits(&vec![0; RECV_MESSAGE_CAPACITY + 1]));
}
