//! RFC-6962 Merkle tree used by headers, txs, commits, and validator sets.
//!
//! Matches `crypto/merkle.HashFromByteSlices` in the Go tree. Proofs are later.

use crate::tmhash::{SIZE, sum};

const LEAF_PREFIX: u8 = 0x00;
const INNER_PREFIX: u8 = 0x01;

/// `merkle.HashFromByteSlices`.
///
/// Empty input is `tmhash([])`. One item is `tmhash(0x00 || item)`.
/// Larger inputs split at the largest power of two strictly below `len`.
#[must_use]
pub fn hash_from_byte_slices<T: AsRef<[u8]>>(items: &[T]) -> [u8; SIZE] {
    match items.len() {
        0 => sum(&[]),
        1 => leaf_hash(items[0].as_ref()),
        len => {
            let k = split_point(len);
            let left = hash_from_byte_slices(&items[..k]);
            let right = hash_from_byte_slices(&items[k..]);
            inner_hash(&left, &right)
        }
    }
}

fn leaf_hash(leaf: &[u8]) -> [u8; SIZE] {
    let mut buf = Vec::with_capacity(1 + leaf.len());
    buf.push(LEAF_PREFIX);
    buf.extend_from_slice(leaf);
    sum(&buf)
}

fn inner_hash(left: &[u8], right: &[u8]) -> [u8; SIZE] {
    let mut buf = Vec::with_capacity(1 + left.len() + right.len());
    buf.push(INNER_PREFIX);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    sum(&buf)
}

/// `merkle.getSplitPoint`: largest power of two strictly less than `length`.
fn split_point(length: usize) -> usize {
    debug_assert!(length > 1);
    let bitlen = usize::BITS - length.leading_zeros();
    let mut k = 1usize << (bitlen - 1);
    if k == length {
        k >>= 1;
    }
    k
}

#[cfg(test)]
mod tests {
    use super::{inner_hash, split_point};

    #[test]
    fn rfc6962_inner_node() {
        // echo -n 014E3132334E343536 | xxd -r -p | sha256sum
        let got = inner_hash(b"N123", b"N456");
        assert_eq!(
            hex::encode(got),
            "aa217fe888e47007fa15edab33c2b492a722cb106c64667fc2b044444de66bbb"
        );
    }

    #[test]
    fn split_matches_go_powers_of_two() {
        assert_eq!(split_point(2), 1);
        assert_eq!(split_point(3), 2);
        assert_eq!(split_point(4), 2);
        assert_eq!(split_point(5), 4);
        assert_eq!(split_point(7), 4);
        assert_eq!(split_point(8), 4);
    }
}
