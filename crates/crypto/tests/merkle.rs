//! RFC-6962 vectors from `crypto/merkle/rfc6962_test.go` and proof cases from
//! `crypto/merkle/tree_test.go` and `proof_test.go`.

use eld_tendermint_crypto::{
    Error, MAX_AUNTS, Proof, hash_from_byte_slices, proofs_from_byte_slices,
};

#[test]
fn rfc6962_empty_tree_is_sha256_of_empty() {
    assert_eq!(
        hex::encode(hash_from_byte_slices(&[] as &[&[u8]])),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn rfc6962_empty_leaf_differs_from_empty_tree() {
    let empty_leaf: &[&[u8]] = &[&[]];
    assert_eq!(
        hex::encode(hash_from_byte_slices(empty_leaf)),
        "6e340b9cffb37a989ca544e6bb780a2c78901d3fb33738768511a30617afa01d"
    );
}

#[test]
fn rfc6962_leaf() {
    let leaves: &[&[u8]] = &[b"L123456"];
    assert_eq!(
        hex::encode(hash_from_byte_slices(leaves)),
        "395aa064aa4c29f7010acfe3f25db9485bbd4b91897b6ad7ad547639252b4d56"
    );
}

#[test]
fn rfc6962_hasher_is_order_and_preimage_sensitive() {
    let hello: &[&[u8]] = &[b"Hello"];
    let world: &[&[u8]] = &[b"World"];
    let hash1 = hash_from_byte_slices(hello);
    let hash2 = hash_from_byte_slices(world);
    assert_ne!(hash1, hash2);

    let pair = [hash1.as_slice(), hash2.as_slice()];
    let sub = hash_from_byte_slices(&pair);
    let swapped = [hash2.as_slice(), hash1.as_slice()];
    assert_ne!(sub, hash_from_byte_slices(&swapped));

    let mut concatenated = Vec::new();
    concatenated.extend_from_slice(&hash1);
    concatenated.extend_from_slice(&hash2);
    let forged: &[&[u8]] = &[&concatenated];
    assert_ne!(sub, hash_from_byte_slices(forged));
}

fn assert_proofs_match_root(items: &[Vec<u8>]) {
    let (root, proofs) = proofs_from_byte_slices(items);
    assert_eq!(root, hash_from_byte_slices(items));
    assert_eq!(proofs.len(), items.len());
    for (index, (item, proof)) in items.iter().zip(&proofs).enumerate() {
        assert_eq!(proof.index, i64::try_from(index).unwrap());
        assert_eq!(proof.total, i64::try_from(items.len()).unwrap());
        proof.verify(&root, item).unwrap();

        if !proof.aunts.is_empty() {
            let mut longer = proof.clone();
            longer.aunts.push(vec![0xAB; 32]);
            assert_eq!(longer.verify(&root, item), Err(Error::ProofRootMismatch));

            let mut shorter = proof.clone();
            shorter.aunts.pop();
            assert_eq!(shorter.verify(&root, item), Err(Error::ProofRootMismatch));
        }

        let mut mutated = item.clone();
        if mutated.is_empty() {
            mutated.push(1);
        } else {
            mutated[0] ^= 0x01;
        }
        assert_eq!(proof.verify(&root, &mutated), Err(Error::ProofLeafMismatch));

        let mut bad_root = root;
        bad_root[0] ^= 0x01;
        assert_eq!(proof.verify(&bad_root, item), Err(Error::ProofRootMismatch));

        let mut bad_index = proof.clone();
        bad_index.index = bad_index.total;
        assert_eq!(bad_index.verify(&root, item), Err(Error::ProofRootMismatch));
    }
}

#[test]
fn proofs_match_hash_for_empty_one_odd_and_many() {
    assert_proofs_match_root(&[]);
    let (root, proofs) = proofs_from_byte_slices(&[] as &[Vec<u8>]);
    assert!(proofs.is_empty());
    assert_eq!(
        hex::encode(root),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    assert_proofs_match_root(&[b"only".to_vec()]);
    assert_proofs_match_root(&[b"apple".to_vec(), b"watermelon".to_vec(), b"kiwi".to_vec()]);
    let many: Vec<Vec<u8>> = (0..100)
        .map(|i| vec![u8::try_from(i).unwrap(); 32])
        .collect();
    assert_proofs_match_root(&many);
}

#[test]
fn proof_validate_basic_matches_go() {
    let (_root, mut proofs) =
        proofs_from_byte_slices(&[b"apple".to_vec(), b"watermelon".to_vec(), b"kiwi".to_vec()]);
    let proof = &mut proofs[0];
    proof.validate_basic().unwrap();

    proof.total = -1;
    assert_eq!(proof.validate_basic(), Err(Error::NegativeProofTotal));
    proof.total = 3;

    proof.index = -1;
    assert_eq!(proof.validate_basic(), Err(Error::NegativeProofIndex));
    proof.index = 0;

    proof.leaf_hash = vec![0; 10];
    assert_eq!(
        proof.validate_basic(),
        Err(Error::InvalidLeafHash { got: 10 })
    );
    proof.leaf_hash =
        proofs_from_byte_slices(&[b"apple".to_vec(), b"watermelon".to_vec(), b"kiwi".to_vec()])
            .1
            .remove(0)
            .leaf_hash;

    proof.aunts = vec![vec![0; 32]; MAX_AUNTS + 1];
    assert_eq!(
        proof.validate_basic(),
        Err(Error::TooManyAunts { got: MAX_AUNTS + 1 })
    );

    proof.aunts = vec![vec![0; 10]];
    assert_eq!(
        proof.validate_basic(),
        Err(Error::InvalidAuntHash { index: 0, got: 10 })
    );
}

#[test]
fn proof_proto_round_trip() {
    let (_root, proofs) =
        proofs_from_byte_slices(&[b"apple".to_vec(), b"watermelon".to_vec(), b"kiwi".to_vec()]);
    let proto = proofs[0].to_proto();
    let back = Proof::try_from_proto(Some(&proto)).unwrap();
    assert_eq!(back, proofs[0]);

    assert_eq!(Proof::try_from_proto(None), Err(Error::MissingProof));
    let empty = eld_tendermint_proto::crypto::Proof {
        total: 0,
        index: 0,
        leaf_hash: Vec::new(),
        aunts: Vec::new(),
    };
    assert_eq!(
        Proof::try_from_proto(Some(&empty)),
        Err(Error::InvalidLeafHash { got: 0 })
    );
}
