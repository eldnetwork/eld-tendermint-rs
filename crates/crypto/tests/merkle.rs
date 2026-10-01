//! RFC-6962 vectors from `crypto/merkle/rfc6962_test.go`.

use eld_tendermint_crypto::hash_from_byte_slices;

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
