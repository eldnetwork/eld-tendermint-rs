//! `libs/bits` cases from `bit_array_test.go`.

use eld_tendermint_proto::libs::bits::BitArray as ProtoBitArray;
use eld_tendermint_types::BitArray;

fn from_pattern(pattern: &str) -> BitArray {
    let json = format!("\"{pattern}\"");
    serde_json::from_str(&json).unwrap()
}

fn pattern(bits: usize, set: &[usize]) -> String {
    let mut chars = vec![b'_'; bits];
    for index in set {
        chars[*index] = b'x';
    }
    String::from_utf8(chars).unwrap()
}

#[test]
fn new_zero_and_negative_are_none() {
    assert!(BitArray::new(0).is_none());
    for bits in [-127_i64, -128, -1, i64::from(i32::MIN)] {
        assert!(BitArray::new(bits).is_none(), "{bits}");
    }

    let one = BitArray::new(1).unwrap();
    assert_eq!(one.size(), 1);
    assert_eq!(one.to_proto().unwrap().elems, vec![0]);
    assert_eq!(
        BitArray::new(64).unwrap().to_proto().unwrap().elems.len(),
        1
    );
    assert_eq!(
        BitArray::new(65).unwrap().to_proto().unwrap().elems.len(),
        2
    );
}

#[test]
fn set_and_get_edges() {
    let mut bit_array = BitArray::new(65).unwrap();
    assert!(!bit_array.get_index(-1));
    assert!(!bit_array.get_index(65));
    assert!(!bit_array.set_index(-1, true));
    assert!(!bit_array.set_index(65, true));
    assert_eq!(bit_array.to_proto().unwrap().elems, vec![0, 0]);

    assert!(bit_array.set_index(0, true));
    assert!(bit_array.get_index(0));
    assert!(!bit_array.get_index(1));
    assert!(bit_array.set_index(64, true));
    assert!(bit_array.get_index(64));
    assert!(!bit_array.get_index(63));
    assert!(bit_array.set_index(0, false));
    assert!(!bit_array.get_index(0));
    assert!(bit_array.get_index(64));
}

#[test]
fn and_or_size_mismatch_and_nil() {
    let left = from_pattern(&pattern(51, &[0, 10, 30, 50]));
    let right = from_pattern(&pattern(31, &[0, 5, 30]));

    let and = BitArray::and(Some(&left), Some(&right)).unwrap();
    assert_eq!(and.size(), 31);
    assert_eq!(
        and.to_proto().unwrap().elems.len(),
        right.to_proto().unwrap().elems.len()
    );
    for i in 0..and.size() {
        assert_eq!(
            and.get_index(i),
            left.get_index(i) && right.get_index(i),
            "{i}"
        );
    }

    let or = BitArray::or(Some(&left), Some(&right)).unwrap();
    assert_eq!(or.size(), 51);
    assert_eq!(
        or.to_proto().unwrap().elems.len(),
        left.to_proto().unwrap().elems.len()
    );
    for i in 0..or.size() {
        assert_eq!(
            or.get_index(i),
            left.get_index(i) || right.get_index(i),
            "{i}"
        );
    }
    assert_eq!(BitArray::or(Some(&right), Some(&left)).unwrap(), or);

    assert!(BitArray::and(None, Some(&left)).is_none());
    assert!(BitArray::and(Some(&left), None).is_none());
    assert!(BitArray::and(None, None).is_none());
    assert_eq!(BitArray::or(None, Some(&left)).as_ref(), Some(&left));
    assert_eq!(BitArray::or(Some(&left), None).as_ref(), Some(&left));
    assert!(BitArray::or(None, None).is_none());

    let wide = from_pattern(&pattern(70, &[0, 69]));
    let narrow = from_pattern(&pattern(20, &[0, 19]));
    let wide_or = BitArray::or(Some(&narrow), Some(&wide)).unwrap();
    assert_eq!(wide_or.size(), 70);
    assert!(wide_or.get_index(0));
    assert!(wide_or.get_index(19));
    assert!(wide_or.get_index(69));
    assert_eq!(BitArray::or(Some(&wide), Some(&narrow)).unwrap(), wide_or);

    let wide_and = BitArray::and(Some(&wide), Some(&narrow)).unwrap();
    assert_eq!(wide_and.size(), 20);
    assert!(wide_and.get_index(0));
    assert!(!wide_and.get_index(19));
    assert!(!wide_and.get_index(69));
}

#[test]
fn sub_matches_go_table() {
    let cases = [
        ("null", "null", "null"),
        (r#""x""#, "null", "null"),
        ("null", r#""x""#, "null"),
        (r#""x""#, r#""x""#, r#""_""#),
        (r#""xxxxxx""#, r#""x_x_x_""#, r#""_x_x_x""#),
        (r#""x_x_x_""#, r#""xxxxxx""#, r#""______""#),
        (r#""xxxxxx""#, r#""x_x_x_xxxx""#, r#""_x_x_x""#),
        (r#""x_x_x_xxxx""#, r#""xxxxxx""#, r#""______xxxx""#),
        (r#""xxxxxxxxxx""#, r#""x_x_x_""#, r#""_x_x_xxxxx""#),
        (r#""x_x_x_""#, r#""xxxxxxxxxx""#, r#""______""#),
    ];
    for (init, subtracting, expected) in cases {
        let init: Option<BitArray> = serde_json::from_str(init).unwrap();
        let subtracting: Option<BitArray> = serde_json::from_str(subtracting).unwrap();
        let got = BitArray::sub(init.as_ref(), subtracting.as_ref());
        assert_eq!(serde_json::to_string(&got).unwrap(), expected);
    }
}

#[test]
fn empty_and_full() {
    for n in [47_i64, 123, 64, 128] {
        let mut bit_array = BitArray::new(n).unwrap();
        assert!(bit_array.is_empty(), "{n}");
        assert!(!bit_array.is_full(), "{n}");
        for i in 0..n {
            assert!(bit_array.set_index(i, true));
        }
        assert!(bit_array.is_full(), "{n}");
        assert!(!bit_array.is_empty(), "{n}");
        assert!(bit_array.set_index(0, false));
        assert!(!bit_array.is_full(), "{n}");
    }
}

#[test]
fn bytes_match_go() {
    let mut bit_array = BitArray::new(4).unwrap();
    bit_array.set_index(0, true);
    assert_eq!(bit_array.bytes(), [0x01]);
    bit_array.set_index(3, true);
    assert_eq!(bit_array.bytes(), [0x09]);

    let mut bit_array = BitArray::new(9).unwrap();
    assert_eq!(bit_array.bytes(), [0x00, 0x00]);
    bit_array.set_index(7, true);
    assert_eq!(bit_array.bytes(), [0x80, 0x00]);
    bit_array.set_index(8, true);
    assert_eq!(bit_array.bytes(), [0x80, 0x01]);

    let mut bit_array = BitArray::new(16).unwrap();
    assert_eq!(bit_array.bytes(), [0x00, 0x00]);
    bit_array.set_index(7, true);
    assert_eq!(bit_array.bytes(), [0x80, 0x00]);
    bit_array.set_index(8, true);
    assert_eq!(bit_array.bytes(), [0x80, 0x01]);
    bit_array.set_index(9, true);
    assert_eq!(bit_array.bytes(), [0x80, 0x03]);
}

#[test]
fn json_round_trip() {
    let mut one_set = BitArray::new(1).unwrap();
    one_set.set_index(0, true);
    let mut five = BitArray::new(5).unwrap();
    five.set_index(0, true);
    five.set_index(1, true);

    let cases = [
        (None, "null"),
        (BitArray::new(0), "null"),
        (BitArray::new(1), r#""_""#),
        (Some(one_set), r#""x""#),
        (Some(five), r#""xx___""#),
        (Some(from_pattern("x_xx_")), r#""x_xx_""#),
    ];

    for (bit_array, expected) in cases {
        let json = serde_json::to_string(&bit_array).unwrap();
        assert_eq!(json, expected);
        let back: Option<BitArray> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bit_array);
        if let (Some(bit_array), Some(back)) = (&bit_array, &back) {
            assert_eq!(back.to_string(), bit_array.to_string());
            assert_eq!(
                back.to_proto().unwrap().elems,
                bit_array.to_proto().unwrap().elems
            );
        }
    }

    assert!(serde_json::from_str::<BitArray>("\"\"").is_err());
    assert!(serde_json::from_str::<BitArray>("\"y\"").is_err());
    assert!(serde_json::from_str::<BitArray>("1").is_err());
    assert_eq!(BitArray::to_display(None), "nil-BitArray");
    assert_eq!(from_pattern("xx___").to_string(), "BA{5:xx___}");
}

#[test]
fn proto_round_trip() {
    assert!(BitArray::try_from_proto(None).unwrap().is_none());
    for n in [1_i64, 2] {
        let bit_array = BitArray::new(n).unwrap();
        let proto = bit_array.to_proto().unwrap();
        assert_eq!(proto.bits, n);
        assert_eq!(proto.elems, vec![0]);
        let back = BitArray::try_from_proto(Some(&proto)).unwrap().unwrap();
        assert_eq!(back, bit_array);
    }

    let negative = ProtoBitArray {
        bits: -1,
        elems: Vec::new(),
    };
    assert!(BitArray::try_from_proto(Some(&negative)).is_err());
}

#[test]
fn update_copies_overlapping_words() {
    let mut receiver = from_pattern(&pattern(10, &[0]));
    let other = from_pattern(&pattern(12, &[1]));
    receiver.update(Some(&other));
    assert_eq!(receiver.size(), 10);
    assert!(!receiver.get_index(0));
    assert!(receiver.get_index(1));

    let before = receiver.copy();
    receiver.update(None);
    assert_eq!(receiver, before);

    let mut longer = from_pattern(&pattern(70, &[69]));
    let short = from_pattern(&pattern(10, &[3]));
    longer.update(Some(&short));
    assert!(longer.get_index(3));
    assert!(longer.get_index(69));
    assert_eq!(longer.size(), 70);
}

#[test]
fn not_flips_padding_bits() {
    let bit_array = BitArray::new(1).unwrap();
    let flipped = bit_array.not();
    assert!(flipped.get_index(0));
    assert_eq!(flipped.to_proto().unwrap().elems, vec![u64::MAX]);
    assert_eq!(flipped.not(), bit_array);
}

#[test]
fn string_indented_matches_go_spacing() {
    let ten = BitArray::new(10).unwrap();
    assert_eq!(ten.string_indented(" "), "BA{10:__________ }");

    let hundred = BitArray::new(100).unwrap();
    let groups = "__________I".repeat(4);
    let expected = format!("BA{{100:{groups}__________II{groups}__________III}}");
    assert_eq!(hundred.string_indented("I"), expected);
}
