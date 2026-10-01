//! Vote and proposal sign-byte vectors from the Go tests.

use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{BlockId, PartSetHeader, Proposal, Time, Vote};

fn zero_vote() -> Vote {
    Vote::default()
}

#[test]
fn vote_sign_bytes_match_go_vectors() {
    let cases: &[(&str, Vote, &[u8])] = &[
        (
            "",
            zero_vote(),
            &[
                0x0d, 0x2a, 0x0b, 0x08, 0x80, 0x92, 0xb8, 0xc3, 0x98, 0xfe, 0xff, 0xff, 0xff, 0x01,
            ],
        ),
        (
            "",
            Vote {
                vote_type: SignedMsgType::Precommit,
                height: 1,
                round: 1,
                ..zero_vote()
            },
            &[
                0x21, 0x08, 0x02, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x19, 0x01,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x0b, 0x08, 0x80, 0x92, 0xb8, 0xc3,
                0x98, 0xfe, 0xff, 0xff, 0xff, 0x01,
            ],
        ),
        (
            "",
            Vote {
                vote_type: SignedMsgType::Prevote,
                height: 1,
                round: 1,
                ..zero_vote()
            },
            &[
                0x21, 0x08, 0x01, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x19, 0x01,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x0b, 0x08, 0x80, 0x92, 0xb8, 0xc3,
                0x98, 0xfe, 0xff, 0xff, 0xff, 0x01,
            ],
        ),
        (
            "",
            Vote {
                height: 1,
                round: 1,
                ..zero_vote()
            },
            &[
                0x1f, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x19, 0x01, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x0b, 0x08, 0x80, 0x92, 0xb8, 0xc3, 0x98, 0xfe,
                0xff, 0xff, 0xff, 0x01,
            ],
        ),
        (
            "test_chain_id",
            Vote {
                height: 1,
                round: 1,
                ..zero_vote()
            },
            &[
                0x2e, 0x11, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x19, 0x01, 0x00, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x2a, 0x0b, 0x08, 0x80, 0x92, 0xb8, 0xc3, 0x98, 0xfe,
                0xff, 0xff, 0xff, 0x01, 0x32, 0x0d, 0x74, 0x65, 0x73, 0x74, 0x5f, 0x63, 0x68, 0x61,
                0x69, 0x6e, 0x5f, 0x69, 0x64,
            ],
        ),
    ];

    for (chain_id, vote, want) in cases {
        assert_eq!(vote.sign_bytes(chain_id), *want);
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
