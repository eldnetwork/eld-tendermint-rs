#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Properties for vote sign bytes and part-set hashes.

use eld_tendermint_crypto::{PrivKey, hash_from_byte_slices};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{PartSet, Vote};
use proptest::prelude::*;

proptest! {
    #[test]
    fn vote_sign_bytes_are_stable_and_the_signature_verifies(height in 0i64..10_000, round in 0i32..50) {
        let key = PrivKey::generate();
        let mut vote = Vote {
            vote_type: SignedMsgType::Prevote,
            height,
            round,
            ..Vote::default()
        };
        let first = vote.sign_bytes("chain");
        prop_assert_eq!(vote.sign_bytes("chain"), first.clone());
        vote.sign(&key, "chain").unwrap();
        vote.verify_signature(&key.public_key().unwrap(), "chain").unwrap();
        prop_assert_eq!(vote.sign_bytes("chain"), first);
    }

    #[test]
    fn part_set_hash_is_the_merkle_root(data in proptest::collection::vec(any::<u8>(), 0..200)) {
        let set = PartSet::from_data(&data, 16).unwrap();
        let mut chunks = Vec::new();
        for index in 0..set.total() {
            chunks.push(set.get_part(index).unwrap().bytes.clone());
        }
        let root = hash_from_byte_slices(&chunks);
        prop_assert_eq!(set.hash(), root.as_slice());
        prop_assert_eq!(set.read_bytes().unwrap(), data);
    }
}
