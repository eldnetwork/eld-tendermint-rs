#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `ValidateBasic` matrices from `vote_test.go`, `proposal_test.go`, and `block_test.go`.

use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{
    BlockId, BlockIdFlag, Commit, CommitSig, HASH_SIZE, MAX_SIGNATURE_SIZE, PartSetHeader,
    Proposal, Time, Vote,
};

fn complete_block_id() -> BlockId {
    BlockId {
        hash: vec![0x11; HASH_SIZE],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![0x22; HASH_SIZE],
        },
    }
}

fn good_vote() -> Vote {
    Vote {
        vote_type: SignedMsgType::Precommit,
        height: 1,
        round: 1,
        block_id: BlockId::default(),
        timestamp: Time::GO_ZERO,
        validator_address: vec![0; 20],
        validator_index: 0,
        signature: vec![1],
    }
}

#[test]
fn vote_validate_basic() {
    assert!(good_vote().validate_basic().is_ok());

    let mut vote = good_vote();
    vote.height = -1;
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.round = -1;
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.block_id = BlockId {
        hash: vec![1, 2, 3],
        part_set_header: PartSetHeader {
            total: 111,
            hash: b"blockparts".to_vec(),
        },
    };
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.block_id = BlockId {
        hash: vec![0xab; HASH_SIZE],
        part_set_header: PartSetHeader::default(),
    };
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.validator_address = vec![0; 1];
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.validator_index = -1;
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.signature.clear();
    assert!(vote.validate_basic().is_err());

    let mut vote = good_vote();
    vote.signature = vec![0; MAX_SIGNATURE_SIZE + 1];
    assert!(vote.validate_basic().is_err());
}

#[test]
fn proposal_validate_basic() {
    let good = Proposal::new(4, 2, 2, complete_block_id(), Time::GO_ZERO);
    let mut good = good;
    good.signature = vec![1];
    assert!(good.validate_basic().is_ok());

    let mut proposal = good.clone();
    proposal.proposal_type = SignedMsgType::Precommit;
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good.clone();
    proposal.height = -1;
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good.clone();
    proposal.round = -1;
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good.clone();
    proposal.pol_round = -2;
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good.clone();
    proposal.block_id = BlockId {
        hash: vec![1, 2, 3],
        part_set_header: PartSetHeader {
            total: 111,
            hash: b"blockparts".to_vec(),
        },
    };
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good.clone();
    proposal.signature.clear();
    assert!(proposal.validate_basic().is_err());

    let mut proposal = good;
    proposal.signature = vec![0; MAX_SIGNATURE_SIZE + 1];
    assert!(proposal.validate_basic().is_err());
}

#[test]
fn block_id_validate_basic() {
    let empty = BlockId {
        hash: Vec::new(),
        part_set_header: PartSetHeader {
            total: 1,
            hash: Vec::new(),
        },
    };
    assert!(empty.validate_basic().is_ok());
    assert!(!empty.is_zero());
    assert!(!empty.is_complete());

    let bad_hash = BlockId {
        hash: vec![0],
        part_set_header: empty.part_set_header.clone(),
    };
    assert!(bad_hash.validate_basic().is_err());

    let bad_parts = BlockId {
        hash: Vec::new(),
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![0],
        },
    };
    assert!(bad_parts.validate_basic().is_err());
    assert!(complete_block_id().is_complete());
    assert!(BlockId::default().is_zero());
}

#[test]
fn commit_validate_basic() {
    let sig = CommitSig {
        block_id_flag: BlockIdFlag::Commit,
        validator_address: vec![0; 20],
        timestamp: Time::parse_rfc3339("2019-10-13T16:14:44Z").unwrap(),
        signature: vec![1],
    };
    let commit = Commit {
        height: 2,
        round: 1,
        block_id: complete_block_id(),
        signatures: vec![sig],
    };
    assert!(commit.validate_basic().is_ok());

    let mut bad_sig = commit.clone();
    bad_sig.signatures[0].signature = vec![0];
    assert!(bad_sig.validate_basic().is_ok());

    let mut negative_height = commit.clone();
    negative_height.height = -100;
    assert!(negative_height.validate_basic().is_err());

    let mut negative_round = commit.clone();
    negative_round.round = -100;
    assert!(negative_round.validate_basic().is_err());

    let mut nil_block = commit.clone();
    nil_block.block_id = BlockId::default();
    assert!(nil_block.validate_basic().is_err());

    let mut no_sigs = commit.clone();
    no_sigs.signatures.clear();
    assert!(no_sigs.validate_basic().is_err());

    let absent = Commit {
        signatures: vec![CommitSig::absent()],
        ..commit
    };
    assert!(absent.validate_basic().is_ok());

    let mut absent_with_sig = CommitSig::absent();
    absent_with_sig.signature = vec![1];
    assert!(absent_with_sig.validate_basic().is_err());
}
