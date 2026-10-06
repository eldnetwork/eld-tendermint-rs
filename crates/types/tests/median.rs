#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! `MedianTime` from `spec/consensus/bft-time.md`.

use eld_tendermint_proto::types::BlockIdFlag;
use eld_tendermint_types::{
    BlockId, Commit, CommitSig, Time, Validator, ValidatorSet, median_time,
};

fn validator(tag: u8, power: i64) -> Validator {
    Validator {
        address: vec![tag; 20],
        pub_key: None,
        voting_power: power,
        proposer_priority: 0,
    }
}

fn signed(address: u8, seconds: i64) -> CommitSig {
    CommitSig {
        block_id_flag: BlockIdFlag::Commit,
        validator_address: vec![address; 20],
        timestamp: Time::from_unix_parts(seconds, 0),
        signature: vec![1; 64],
    }
}

#[test]
fn weighted_median_ignores_a_heavy_faulty_tail() {
    // (p2, 27, 98), (p3, 10, 1000), (p4, 10, 500). Median is 98.
    let validators = ValidatorSet::new(vec![validator(2, 27), validator(3, 10), validator(4, 10)])
        .expect("validators");
    let commit = Commit {
        height: 1,
        round: 0,
        block_id: BlockId::default(),
        signatures: vec![signed(2, 98), signed(4, 500), signed(3, 1000)],
    };
    assert_eq!(
        median_time(&commit, &validators),
        Time::from_unix_parts(98, 0)
    );
}

#[test]
fn absent_signatures_add_no_weight() {
    let validators = ValidatorSet::new(vec![validator(1, 10), validator(2, 10)]).expect("vals");
    let commit = Commit {
        height: 1,
        round: 0,
        block_id: BlockId::default(),
        signatures: vec![CommitSig::absent(), signed(2, 50)],
    };
    assert_eq!(
        median_time(&commit, &validators),
        Time::from_unix_parts(50, 0)
    );
}
