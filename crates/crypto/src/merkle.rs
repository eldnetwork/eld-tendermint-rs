//! RFC-6962 Merkle tree used by headers, txs, commits, validator sets, and block parts.
//!
//! Matches `crypto/merkle.HashFromByteSlices` and `merkle.Proof` in the Go tree.

use crate::ensured::Ensured;
use crate::error::Error;
use crate::tmhash::{SIZE, sum};

/// `merkle.MaxAunts`. A tree of size `2^100` is the largest proof this accepts.
pub const MAX_AUNTS: usize = 100;

const LEAF_PREFIX: u8 = 0x00;
const INNER_PREFIX: u8 = 0x01;

/// `merkle.HashFromByteSlices`.
///
/// Empty input is `tmhash([])`. One item is `tmhash(0x00 || item)`.
/// Larger inputs split at the largest power of two strictly below `len`.
///
/// ```
/// use eld_tendermint_crypto::hash_from_byte_slices;
/// let empty: &[&[u8]] = &[];
/// assert_eq!(
///     hash_from_byte_slices(empty),
///     [
///         0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
///         0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
///         0x78, 0x52, 0xb8, 0x55,
///     ]
/// );
/// assert_ne!(
///     hash_from_byte_slices(&[b"Hello".as_slice()]),
///     hash_from_byte_slices(empty)
/// );
/// ```
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

pub(crate) fn leaf_hash(leaf: &[u8]) -> [u8; SIZE] {
    let mut buf = Vec::with_capacity(1 + leaf.len());
    buf.push(LEAF_PREFIX);
    buf.extend_from_slice(leaf);
    sum(&buf)
}

pub(crate) fn inner_hash(left: &[u8], right: &[u8]) -> [u8; SIZE] {
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

/// `merkle.Proof`. Leaf hash is included; the root hash is not.
///
/// Aunts run from the leaf's sibling toward the root. The last aunt is the
/// sibling nearest the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub total: i64,
    pub index: i64,
    pub leaf_hash: Vec<u8>,
    pub aunts: Vec<Vec<u8>>,
}

struct Trail {
    leaf_hash: [u8; SIZE],
    aunts: Vec<Vec<u8>>,
}

/// `merkle.ProofsFromByteSlices`.
///
/// The root matches [`hash_from_byte_slices`] of the same items. Empty input
/// is the empty-tree hash and no proofs.
///
/// # Panics
///
/// Panics when the item count does not fit in `i64`. A tree that large is outside
/// the proof limit of [`MAX_AUNTS`].
#[must_use]
pub fn proofs_from_byte_slices<T: AsRef<[u8]>>(items: &[T]) -> ([u8; SIZE], Vec<Proof>) {
    let (trails, root) = trails_from_byte_slices(items);
    let total = i64::try_from(items.len()).ensured("item count fits in i64");
    let proofs = trails
        .into_iter()
        .enumerate()
        .map(|(index, trail)| Proof {
            total,
            index: i64::try_from(index).ensured("proof index fits in i64"),
            leaf_hash: trail.leaf_hash.to_vec(),
            aunts: trail.aunts,
        })
        .collect();
    (root, proofs)
}

fn trails_from_byte_slices<T: AsRef<[u8]>>(items: &[T]) -> (Vec<Trail>, [u8; SIZE]) {
    match items.len() {
        0 => (Vec::new(), sum(&[])),
        1 => {
            let leaf_hash = leaf_hash(items[0].as_ref());
            (
                vec![Trail {
                    leaf_hash,
                    aunts: Vec::new(),
                }],
                leaf_hash,
            )
        }
        len => {
            let k = split_point(len);
            let (mut lefts, left_hash) = trails_from_byte_slices(&items[..k]);
            let (mut rights, right_hash) = trails_from_byte_slices(&items[k..]);
            for trail in &mut lefts {
                trail.aunts.push(right_hash.to_vec());
            }
            for trail in &mut rights {
                trail.aunts.push(left_hash.to_vec());
            }
            lefts.append(&mut rights);
            (lefts, inner_hash(&left_hash, &right_hash))
        }
    }
}

impl Proof {
    /// `Proof.Verify`. Recomputes `tmhash(0x00 || leaf)` and walks aunts with
    /// the same split as [`hash_from_byte_slices`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::NegativeProofTotal`], [`Error::NegativeProofIndex`],
    /// [`Error::ProofLeafMismatch`], or [`Error::ProofRootMismatch`].
    pub fn verify(&self, root: &[u8], leaf: &[u8]) -> Result<(), Error> {
        if self.total < 0 {
            return Err(Error::NegativeProofTotal);
        }
        if self.index < 0 {
            return Err(Error::NegativeProofIndex);
        }
        let leaf_hash = leaf_hash(leaf);
        if self.leaf_hash.as_slice() != leaf_hash.as_slice() {
            return Err(Error::ProofLeafMismatch);
        }
        match self.compute_root_hash() {
            Some(computed) if computed.as_slice() == root => Ok(()),
            _ => Err(Error::ProofRootMismatch),
        }
    }

    /// `Proof.ComputeRootHash`. `None` when the aunt list has the wrong length
    /// or the index is outside the tree. Does not check the leaf bytes.
    #[must_use]
    pub fn compute_root_hash(&self) -> Option<Vec<u8>> {
        compute_hash_from_aunts(self.index, self.total, &self.leaf_hash, &self.aunts)
    }

    /// `Proof.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns a negative total or index, a leaf hash that is not 32 bytes,
    /// more than [`MAX_AUNTS`] aunts, or an aunt that is not 32 bytes.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.total < 0 {
            return Err(Error::NegativeProofTotal);
        }
        if self.index < 0 {
            return Err(Error::NegativeProofIndex);
        }
        if self.leaf_hash.len() != SIZE {
            return Err(Error::InvalidLeafHash {
                got: self.leaf_hash.len(),
            });
        }
        if self.aunts.len() > MAX_AUNTS {
            return Err(Error::TooManyAunts {
                got: self.aunts.len(),
            });
        }
        for (index, aunt) in self.aunts.iter().enumerate() {
            if aunt.len() != SIZE {
                return Err(Error::InvalidAuntHash {
                    index,
                    got: aunt.len(),
                });
            }
        }
        Ok(())
    }

    /// `Proof.ToProto`.
    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::crypto::Proof {
        eld_tendermint_proto::crypto::Proof {
            total: self.total,
            index: self.index,
            leaf_hash: self.leaf_hash.clone(),
            aunts: self.aunts.clone(),
        }
    }

    /// `ProofFromProto`. Missing proto is an error. Runs [`Self::validate_basic`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingProof`] or the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(
        proto: Option<&eld_tendermint_proto::crypto::Proof>,
    ) -> Result<Self, Error> {
        let Some(proto) = proto else {
            return Err(Error::MissingProof);
        };
        let proof = Self {
            total: proto.total,
            index: proto.index,
            leaf_hash: proto.leaf_hash.clone(),
            aunts: proto.aunts.clone(),
        };
        proof.validate_basic()?;
        Ok(proof)
    }
}

/// `computeHashFromAunts`. The last aunt is the sibling at this level.
fn compute_hash_from_aunts(
    index: i64,
    total: i64,
    leaf_hash: &[u8],
    aunts: &[Vec<u8>],
) -> Option<Vec<u8>> {
    if index >= total || index < 0 || total <= 0 {
        return None;
    }
    if total == 1 {
        if aunts.is_empty() {
            Some(leaf_hash.to_vec())
        } else {
            None
        }
    } else {
        let (sibling, rest) = aunts.split_last()?;
        let total_usize = usize::try_from(total).ok()?;
        let num_left = i64::try_from(split_point(total_usize)).ok()?;
        if index < num_left {
            let left = compute_hash_from_aunts(index, num_left, leaf_hash, rest)?;
            Some(inner_hash(&left, sibling).to_vec())
        } else {
            let right =
                compute_hash_from_aunts(index - num_left, total - num_left, leaf_hash, rest)?;
            Some(inner_hash(sibling, &right).to_vec())
        }
    }
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
