#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Properties for Merkle roots and Amino JSON. Fixed Go vectors stay in the other tests.

use eld_tendermint_crypto::{
    PubKey, hash_from_byte_slices, marshal_pub_key, proofs_from_byte_slices, unmarshal_pub_key,
};
use proptest::prelude::*;

proptest! {
    #[test]
    fn merkle_root_is_stable_and_proofs_verify(
        leaves in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..24), 0..12)
    ) {
        let (root, proofs) = proofs_from_byte_slices(&leaves);
        let again = hash_from_byte_slices(&leaves);
        prop_assert_eq!(root, again);
        prop_assert_eq!(again, hash_from_byte_slices(&leaves));
        prop_assert_eq!(proofs.len(), leaves.len());
        for (item, proof) in leaves.iter().zip(proofs) {
            proof.verify(&root, item).unwrap();
        }
    }

    #[test]
    fn amino_pubkey_keeps_type_and_round_trips(bytes in proptest::array::uniform32(any::<u8>())) {
        let key = PubKey::from_bytes(&bytes).unwrap();
        let json = marshal_pub_key(&key);
        let prefix = "{\"type\":\"tendermint/PubKeyEd25519\",\"value\":\"";
        let suffix = "\"}";
        prop_assert!(json.starts_with(prefix));
        prop_assert!(json.ends_with(suffix));
        prop_assert_eq!(unmarshal_pub_key(&json).unwrap(), key);
    }
}
