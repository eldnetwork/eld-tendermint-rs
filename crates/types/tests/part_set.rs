//! Part set cases from `types/part_set_test.go`, using a known byte string.

use eld_tendermint_crypto::hash_from_byte_slices;
use eld_tendermint_types::{BLOCK_PART_SIZE_BYTES, Error, MAX_BLOCK_PARTS_COUNT, Part, PartSet};

const DATA: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const PART_SIZE: u32 = 8;

fn sample() -> PartSet {
    PartSet::from_data(DATA, PART_SIZE).unwrap()
}

#[test]
fn constants_match_go() {
    assert_eq!(BLOCK_PART_SIZE_BYTES, 65_536);
    assert_eq!(MAX_BLOCK_PARTS_COUNT, 1601);
}

#[test]
fn complete_set_hash_is_merkle_root_of_parts() {
    let part_set = sample();
    assert_eq!(part_set.total(), 4);
    assert!(part_set.is_complete());
    assert_eq!(part_set.count(), 4);
    assert_eq!(part_set.byte_size(), i64::try_from(DATA.len()).unwrap());
    assert_eq!(part_set.bit_array().unwrap().size(), 4);

    let chunks: Vec<&[u8]> = [&DATA[0..8], &DATA[8..16], &DATA[16..24], &DATA[24..26]].into();
    assert_eq!(part_set.get_part(0).unwrap().bytes, chunks[0]);
    assert_eq!(part_set.get_part(3).unwrap().bytes, b"yz");
    assert_eq!(part_set.get_part(3).unwrap().bytes.len(), 2);

    let root = hash_from_byte_slices(&chunks);
    assert_eq!(part_set.hash(), root.as_slice());
    let header = part_set.header();
    assert_eq!(header.hash, root);
    assert!(part_set.has_header(&header));
    part_set
        .get_part(0)
        .unwrap()
        .proof
        .verify(&header.hash, &part_set.get_part(0).unwrap().bytes)
        .unwrap();
    assert_eq!(part_set.read_bytes().unwrap(), DATA);

    let empty = PartSet::from_data(b"", PART_SIZE).unwrap();
    assert!(empty.is_complete());
    assert!(empty.bit_array().is_none());
    assert_eq!(
        empty.hash(),
        hash_from_byte_slices(&[] as &[&[u8]]).as_slice()
    );
    assert!(empty.read_bytes().unwrap().is_empty());
    assert!(PartSet::from_data(DATA, 0).is_err());
}

#[test]
fn rebuild_from_header_and_reject_bad_parts() {
    let part_set = sample();
    let mut rebuilt = PartSet::from_header(part_set.header());
    assert!(rebuilt.has_header(&part_set.header()));
    assert!(!rebuilt.is_complete());
    assert_eq!(rebuilt.read_bytes(), Err(Error::IncompletePartSet));

    for index in 0..part_set.total() {
        assert!(
            rebuilt
                .add_part(part_set.get_part(index).unwrap().clone())
                .unwrap()
        );
    }
    assert_eq!(
        rebuilt.add_part(part_set.get_part(0).unwrap().clone()),
        Ok(false)
    );
    assert_eq!(
        rebuilt.add_part(Part {
            index: 10_000,
            bytes: Vec::new(),
            proof: part_set.get_part(0).unwrap().proof.clone(),
        }),
        Err(Error::UnexpectedPartIndex {
            index: 10_000,
            total: 4
        })
    );
    assert!(rebuilt.is_complete());
    assert_eq!(rebuilt.hash(), part_set.hash());
    assert_eq!(rebuilt.read_bytes().unwrap(), DATA);

    let mut wrong_proof = PartSet::from_header(part_set.header());
    let mut part = part_set.get_part(0).unwrap().clone();
    part.proof.aunts[0][0] ^= 0x01;
    assert_eq!(wrong_proof.add_part(part), Err(Error::InvalidPartProof));

    let mut wrong_bytes = PartSet::from_header(part_set.header());
    let mut part = part_set.get_part(1).unwrap().clone();
    part.bytes[0] ^= 0x01;
    assert_eq!(wrong_bytes.add_part(part), Err(Error::InvalidPartProof));
}

#[test]
fn part_validate_basic_and_proto() {
    let part_set = sample();
    let mut part = part_set.get_part(0).unwrap().clone();
    part.validate_basic().unwrap();

    part.bytes = vec![0; usize::try_from(BLOCK_PART_SIZE_BYTES).unwrap() + 1];
    assert!(matches!(
        part.validate_basic(),
        Err(Error::PartTooBig { .. })
    ));

    part.bytes = vec![1, 2, 3];
    part.proof.leaf_hash = vec![0; 10];
    assert!(part.validate_basic().is_err());

    let original = part_set.get_part(0).unwrap();
    let proto = original.to_proto();
    let back = Part::try_from_proto(Some(&proto)).unwrap();
    assert_eq!(&back, original);
    assert_eq!(Part::try_from_proto(None), Err(Error::MissingPart));
    assert!(Part::try_from_proto(Some(&eld_tendermint_proto::types::Part::default())).is_err());
}
