#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Vote and proposal sign-byte vectors from the Go tests.

use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{BlockId, PartSetHeader, Proposal, Time, Vote};

fn zero_vote() -> Vote {
    Vote::default()
}

#[test]
fn vote_sign_bytes_match_go_vectors() {
    let expected: Vec<&str> = include_str!("../../../tests/vectors/vote-sign-bytes.hex")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let cases = [
        ("", zero_vote()),
        (
            "",
            Vote {
                vote_type: SignedMsgType::Precommit,
                height: 1,
                round: 1,
                ..zero_vote()
            },
        ),
        (
            "",
            Vote {
                vote_type: SignedMsgType::Prevote,
                height: 1,
                round: 1,
                ..zero_vote()
            },
        ),
        (
            "",
            Vote {
                height: 1,
                round: 1,
                ..zero_vote()
            },
        ),
        (
            "test_chain_id",
            Vote {
                height: 1,
                round: 1,
                ..zero_vote()
            },
        ),
    ];
    assert_eq!(cases.len(), expected.len());
    for ((chain_id, vote), want) in cases.iter().zip(expected) {
        assert_eq!(hex::encode(vote.sign_bytes(chain_id)), want);
    }
}

#[test]
fn proposal_sign_bytes_match_go() {
    // `TestProposalSignable`: type is left at the Go zero value. Canonical bytes still
    // force SIGNED_MSG_TYPE_PROPOSAL.
    let hash = b"--June_15_2020_amino_was_removed".to_vec();
    let mut proposal = Proposal::new(
        12345,
        23456,
        -1,
        BlockId {
            hash: hash.clone(),
            part_set_header: PartSetHeader { total: 111, hash },
        },
        Time::parse_rfc3339("2018-02-11T07:09:22.765Z").unwrap(),
    );
    proposal.proposal_type = SignedMsgType::Unknown;

    assert_eq!(
        hex::encode(proposal.sign_bytes("test_chain_id")),
        "8601082011393000000000000019a05b00000000000020ffffffffffffffffff012a480a202d2d4a756e655f31355f323032305f616d696e6f5f7761735f72656d6f7665641224086f12202d2d4a756e655f31355f323032305f616d696e6f5f7761735f72656d6f766564320c08a2d8ffd30510c0f2e3ec023a0d746573745f636861696e5f6964"
    );
}

#[test]
fn zero_block_id_is_omitted_from_proposal_sign_bytes() {
    let with_block = Proposal::new(
        1,
        1,
        -1,
        BlockId {
            hash: vec![9; 32],
            part_set_header: PartSetHeader {
                total: 1,
                hash: vec![8; 32],
            },
        },
        Time::GO_ZERO,
    );
    let without = Proposal {
        block_id: BlockId::default(),
        ..with_block.clone()
    };
    assert!(without.sign_bytes("chain").len() < with_block.sign_bytes("chain").len());
}

#[test]
fn signed_vote_verifies_with_the_derived_pubkey() {
    let priv_key = eld_tendermint_crypto::PrivKey::generate();
    let pub_key = priv_key.public_key().unwrap();
    assert_eq!(&priv_key.as_bytes()[32..], pub_key.as_bytes().as_slice());

    let mut vote = Vote {
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
        validator_address: pub_key.address().to_vec(),
        validator_index: 0,
        signature: Vec::new(),
    };
    let sign_bytes = vote.sign_bytes("chain");
    vote.sign(&priv_key, "chain").unwrap();
    assert_eq!(vote.signature.len(), 64);
    vote.verify_signature(&pub_key, "chain").unwrap();

    vote.signature[0] ^= 0x01;
    assert!(vote.verify_signature(&pub_key, "chain").is_err());
    vote.signature[0] ^= 0x01;
    assert!(vote.verify_signature(&pub_key, "other-chain").is_err());
    assert_eq!(vote.sign_bytes("chain"), sign_bytes);

    let mut short = vote.clone();
    short.signature = vec![1; 63];
    assert!(short.verify_signature(&pub_key, "chain").is_err());
}
