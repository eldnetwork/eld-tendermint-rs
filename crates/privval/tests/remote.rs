//! A dialed signer returns the FilePV signature and rejects a conflicting second sign.

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use eld_tendermint_crypto::{PrivKey, pub_key_to_proto, sum, sum_truncated};
use eld_tendermint_p2p::make_secret_connection;
use eld_tendermint_privval::{
    Error, FilePV, PrivValidator, RemoteSigner, read_delimited, sign_vote_message, write_delimited,
};
use eld_tendermint_proto::privval::{
    Message as PvMessage, PubKeyResponse, RemoteSignerError, SignedVoteResponse, message::Sum,
};
use eld_tendermint_types::{BlockId, PartSetHeader, SignedMsgType, Time, Vote};
use prost::Message;

const CHAIN_ID: &str = "remote-chain";

#[test]
fn sign_vote_request_matches_the_privval_vector() {
    let vote = Vote {
        vote_type: SignedMsgType::Prevote,
        height: 3,
        round: 2,
        block_id: BlockId {
            hash: sum(b"blockID_hash").to_vec(),
            part_set_header: PartSetHeader {
                total: 1_000_000,
                hash: sum(b"blockID_part_set_header_hash").to_vec(),
            },
        },
        timestamp: Time::from_unix_parts(1_570_983_284, 0),
        validator_address: sum_truncated(b"validator_address").to_vec(),
        validator_index: 56_789,
        signature: Vec::new(),
    };
    let bytes = sign_vote_message("", &vote).encode_to_vec();
    assert_eq!(
        hex::encode(bytes),
        "1a760a74080110031802224a0a208b01023386c371778ecb6368573e539afc3cc860ec3a2f614e54fe5652f4fc80122608c0843d122072db3d959635dff1bb567bedaa70573392c5159666a3f8caf11e413aac52207a2a0608f49a8ded0532146af1f4111082efb388211bc72c55bcd61e9ac3d538d5bb03"
    );
}

#[test]
fn dialed_signer_returns_the_file_pv_signature_and_rejects_a_conflict() {
    let dir = temp_dir("remote");
    let key = dir.join("key.json");
    let state = dir.join("state.json");
    let state_local = dir.join("state-local.json");
    FilePV::generate(&key, &state).save().unwrap();
    fs::copy(&state, &state_local).unwrap();
    let server_pv = FilePV::load(&key, &state).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || serve(listener, server_pv));

    let mut remote = RemoteSigner::dial(&addr.to_string(), CHAIN_ID).unwrap();
    let mut vote = sample_vote();
    let mut expected = vote.clone();
    let mut local = FilePV::load(&key, &state_local).unwrap();
    local.sign_vote(CHAIN_ID, &mut expected).unwrap();
    remote.sign_vote(CHAIN_ID, &mut vote).unwrap();
    assert_eq!(vote.signature, expected.signature);

    let stored = vote.signature.clone();
    remote.sign_vote(CHAIN_ID, &mut vote).unwrap();
    assert_eq!(vote.signature, stored);

    vote.block_id.hash = vec![9; 32];
    vote.signature.clear();
    let err = remote.sign_vote(CHAIN_ID, &mut vote).unwrap_err();
    assert!(matches!(err, Error::ConflictingData), "{err}");
}

fn serve(listener: TcpListener, mut pv: FilePV) {
    let stream = listener.accept().unwrap().0;
    let _ = stream.set_nodelay(true);
    let key = PrivKey::generate();
    let mut conn = make_secret_connection(stream, &key).unwrap();
    loop {
        let request = match read_delimited(&mut conn) {
            Ok(request) => request,
            Err(_) => return,
        };
        let response = answer(&mut pv, request);
        if write_delimited(&mut conn, &response).is_err() {
            return;
        }
    }
}

fn answer(pv: &mut FilePV, request: PvMessage) -> PvMessage {
    match request.sum {
        Some(Sum::PubKeyRequest(req)) if req.chain_id == CHAIN_ID => PvMessage {
            sum: Some(Sum::PubKeyResponse(PubKeyResponse {
                pub_key: Some(pub_key_to_proto(&pv.get_pub_key())),
                error: None,
            })),
        },
        Some(Sum::PubKeyRequest(_)) => pubkey_error("unable to provide pubkey"),
        Some(Sum::SignVoteRequest(req)) if req.chain_id == CHAIN_ID => {
            let Some(proto) = req.vote else {
                return vote_error("missing vote");
            };
            let Ok(mut vote) = vote_from_proto(&proto) else {
                return vote_error("invalid vote");
            };
            match pv.sign_vote(CHAIN_ID, &mut vote) {
                Ok(()) => PvMessage {
                    sum: Some(Sum::SignedVoteResponse(SignedVoteResponse {
                        vote: Some(vote.to_proto()),
                        error: None,
                    })),
                },
                Err(err) => vote_error(&err.to_string()),
            }
        }
        Some(Sum::SignVoteRequest(_)) => vote_error("unable to sign vote"),
        _ => PvMessage { sum: None },
    }
}

fn vote_from_proto(proto: &eld_tendermint_proto::types::Vote) -> Result<Vote, ()> {
    let vote_type = match proto.r#type {
        1 => SignedMsgType::Prevote,
        2 => SignedMsgType::Precommit,
        _ => return Err(()),
    };
    let block_id = match &proto.block_id {
        Some(block_id) => BlockId::try_from_proto(block_id).map_err(|_| ())?,
        None => BlockId::default(),
    };
    Ok(Vote {
        vote_type,
        height: proto.height,
        round: proto.round,
        block_id,
        timestamp: Time::from_prost(proto.timestamp.as_ref()),
        validator_address: proto.validator_address.clone(),
        validator_index: proto.validator_index,
        signature: proto.signature.clone(),
    })
}

fn pubkey_error(description: &str) -> PvMessage {
    PvMessage {
        sum: Some(Sum::PubKeyResponse(PubKeyResponse {
            pub_key: None,
            error: Some(remote_error(description)),
        })),
    }
}

fn vote_error(description: &str) -> PvMessage {
    PvMessage {
        sum: Some(Sum::SignedVoteResponse(SignedVoteResponse {
            vote: None,
            error: Some(remote_error(description)),
        })),
    }
}

fn remote_error(description: &str) -> RemoteSignerError {
    RemoteSignerError {
        code: 0,
        description: description.to_owned(),
    }
}

fn sample_vote() -> Vote {
    Vote {
        vote_type: SignedMsgType::Prevote,
        height: 1,
        round: 0,
        block_id: BlockId {
            hash: vec![1; 32],
            part_set_header: PartSetHeader {
                total: 1,
                hash: vec![2; 32],
            },
        },
        timestamp: Time::from_unix_parts(1_700_000_000, 0),
        validator_address: vec![3; 20],
        validator_index: 0,
        signature: Vec::new(),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "eld-pv-remote-{name}-{}-{nanos}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}
