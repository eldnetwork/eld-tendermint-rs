//! `libs/bits.BitArray` from Tendermint 0.34.
//!
//! Go locks a mutex on every method. This type is not internally synchronized.
//! [`BitArray::pick_random`] uses the `rand` crate in place of Tendermint's process-wide RNG.

use rand::Rng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Error;

/// Thread-unsafe port of `bits.BitArray`.
///
/// `None` is Go's nil. [`BitArray::new`] returns `None` when `bits <= 0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitArray {
    bits: i64,
    elems: Vec<u64>,
}

impl BitArray {
    /// `NewBitArray`. `None` when `bits <= 0` (Go returns nil).
    #[must_use]
    pub fn new(bits: i64) -> Option<Self> {
        let len = word_len(bits)?;
        Some(Self {
            bits,
            elems: vec![0; len],
        })
    }

    /// `BitArray.Size`.
    #[must_use]
    pub const fn size(&self) -> i64 {
        self.bits
    }

    /// `BitArray.GetIndex`. Out-of-range indexes, including negatives, are `false`.
    #[must_use]
    pub fn get_index(&self, i: i64) -> bool {
        let Some(i) = in_range(self.bits, i) else {
            return false;
        };
        self.elems[i / 64] & (1_u64 << (i % 64)) != 0
    }

    /// `BitArray.SetIndex`. Returns `false` when `i` is out of range.
    pub fn set_index(&mut self, i: i64, value: bool) -> bool {
        let Some(i) = in_range(self.bits, i) else {
            return false;
        };
        let mask = 1_u64 << (i % 64);
        if value {
            self.elems[i / 64] |= mask;
        } else {
            self.elems[i / 64] &= !mask;
        }
        true
    }

    /// `BitArray.Copy`.
    #[must_use]
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// `BitArray.Not`. Complements every word, including unused high bits.
    #[must_use]
    pub fn not(&self) -> Self {
        let mut copy = self.copy();
        for elem in &mut copy.elems {
            *elem = !*elem;
        }
        copy
    }

    /// `BitArray.Or`.
    ///
    /// The result has `max` bits. The shorter array is zero-padded on the right.
    /// One `None` returns a copy of the other. Both `None` returns `None`.
    #[must_use]
    pub fn or(this: Option<&Self>, other: Option<&Self>) -> Option<Self> {
        match (this, other) {
            (None, None) => None,
            (Some(bit_array), None) | (None, Some(bit_array)) => Some(bit_array.copy()),
            (Some(this), Some(other)) => {
                let (base, extra) = if this.bits >= other.bits {
                    (this, other)
                } else {
                    (other, this)
                };
                let mut combined = base.copy_bits(base.bits);
                let words = combined.elems.len().min(extra.elems.len());
                for i in 0..words {
                    combined.elems[i] |= extra.elems[i];
                }
                Some(combined)
            }
        }
    }

    /// `BitArray.And`.
    ///
    /// The result has `min` bits. Either `None` returns `None`.
    #[must_use]
    pub fn and(this: Option<&Self>, other: Option<&Self>) -> Option<Self> {
        match (this, other) {
            (Some(this), Some(other)) => {
                let bits = this.bits.min(other.bits);
                let mut combined = this.copy_bits(bits);
                for (word, other_word) in combined.elems.iter_mut().zip(&other.elems) {
                    *word &= *other_word;
                }
                Some(combined)
            }
            _ => None,
        }
    }

    /// `BitArray.Sub`. Carryless subtraction (`a &^ b`).
    ///
    /// The result has the receiver's bit count. A shorter argument is zero-padded.
    /// Bits past the receiver are ignored. Either `None` returns `None`.
    #[must_use]
    pub fn sub(this: Option<&Self>, other: Option<&Self>) -> Option<Self> {
        match (this, other) {
            (Some(this), Some(other)) => {
                let mut combined = this.copy_bits(this.bits);
                let words = combined.elems.len().min(other.elems.len());
                for i in 0..words {
                    combined.elems[i] &= !other.elems[i];
                }
                Some(combined)
            }
            _ => None,
        }
    }

    /// `BitArray.PickRandom`. `None` when no live bit is set, including a nil array's caller.
    ///
    /// The index is chosen from the set bits the way `getTrueIndices` lists them.
    #[must_use]
    pub fn pick_random(&self) -> Option<i64> {
        let indices = self.true_indices();
        if indices.is_empty() {
            return None;
        }
        let n = rand::thread_rng().gen_range(0..indices.len());
        Some(indices[n])
    }

    /// [`Self::pick_random`] with the choice fixed to `n` in the set-bit list.
    ///
    /// `None` when no bit is set or `n` is past the end of that list.
    #[must_use]
    pub fn pick_random_at(&self, n: usize) -> Option<i64> {
        self.true_indices().get(n).copied()
    }

    /// `BitArray.getTrueIndices`. Padding bits in the last word are ignored.
    #[must_use]
    fn true_indices(&self) -> Vec<i64> {
        if self.elems.is_empty() {
            return Vec::new();
        }
        let mut indices = Vec::new();
        let mut cur_bit = 0_i64;
        let last = self.elems.len() - 1;
        for elem in &self.elems[..last] {
            if *elem == 0 {
                cur_bit += 64;
                continue;
            }
            for shift in 0..64 {
                if elem & (1_u64 << shift) > 0 {
                    indices.push(cur_bit);
                }
                cur_bit += 1;
            }
        }
        let last_elem = self.elems[last];
        let num_final = self.bits - cur_bit;
        for shift in 0..num_final {
            if last_elem & (1_u64 << shift) > 0 {
                indices.push(cur_bit);
            }
            cur_bit += 1;
        }
        indices
    }

    /// `BitArray.IsEmpty`. `true` when every word is zero, including padding bits.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.elems.iter().all(|elem| *elem == 0)
    }

    /// `BitArray.IsFull`. Only live bits must be set; padding in the last word is ignored.
    ///
    /// A multiple of 64 uses Go's unsigned shift: a shift of 64 yields 0, so the mask is
    /// `u64::MAX` rather than Rust's masked shift.
    #[must_use]
    pub fn is_full(&self) -> bool {
        let Some((last, head)) = self.elems.split_last() else {
            return false;
        };
        if head.iter().any(|elem| *elem != u64::MAX) {
            return false;
        }
        let last_elem_bits = (self.bits + 63) % 64 + 1;
        let mask = if last_elem_bits == 64 {
            u64::MAX
        } else {
            (1_u64 << last_elem_bits) - 1
        };
        last.wrapping_add(1) & mask == 0
    }

    /// `BitArray.Bytes`. Little-endian words, truncated to `(bits + 7) / 8` bytes.
    #[must_use]
    pub fn bytes(&self) -> Vec<u8> {
        let num_bytes = usize::try_from((self.bits + 7) / 8).expect("byte length fits");
        let mut bytes = vec![0_u8; num_bytes];
        for (i, elem) in self.elems.iter().enumerate() {
            let start = i * 8;
            if start >= bytes.len() {
                break;
            }
            let raw = elem.to_le_bytes();
            let n = raw.len().min(bytes.len() - start);
            bytes[start..start + n].copy_from_slice(&raw[..n]);
        }
        bytes
    }

    /// `BitArray.Update`. Copies `min` words from `other` and does not change the size.
    ///
    /// `None` is a no-op. A nil receiver is a no-op because there is no value to update.
    pub fn update(&mut self, other: Option<&Self>) {
        let Some(other) = other else {
            return;
        };
        let n = self.elems.len().min(other.elems.len());
        self.elems[..n].copy_from_slice(&other.elems[..n]);
    }

    /// `BitArray.StringIndented`.
    ///
    /// Inserts `indent` after every 10th bit and again after every 50th. Every 100th bit
    /// starts a new line before those indents, so they land on the next line.
    #[must_use]
    pub fn string_indented(&self, indent: &str) -> String {
        let mut lines = Vec::new();
        let mut bits = String::new();
        for i in 0..self.bits {
            bits.push(if self.get_index(i) { 'x' } else { '_' });
            if i % 100 == 99 {
                lines.push(std::mem::take(&mut bits));
            }
            if i % 10 == 9 {
                bits.push_str(indent);
            }
            if i % 50 == 49 {
                bits.push_str(indent);
            }
        }
        if !bits.is_empty() {
            lines.push(bits);
        }
        format!("BA{{{}:{}}}", self.bits, lines.join(indent))
    }

    /// Go `String()` for a value or nil (`"nil-BitArray"`).
    #[must_use]
    pub fn to_display(bit_array: Option<&Self>) -> String {
        match bit_array {
            Some(bit_array) => bit_array.string_indented(""),
            None => "nil-BitArray".to_owned(),
        }
    }

    /// `BitArray.ToProto`. `None` when there are no words, matching Go's nil proto.
    #[must_use]
    pub fn to_proto(&self) -> Option<eld_tendermint_proto::libs::bits::BitArray> {
        if self.elems.is_empty() {
            None
        } else {
            Some(eld_tendermint_proto::libs::bits::BitArray {
                bits: self.bits,
                elems: self.elems.clone(),
            })
        }
    }

    /// `BitArray.FromProto`.
    ///
    /// Missing proto is `Ok(None)`. `bits == 0` with no words is `Ok(None)`.
    /// A negative count, an overflowing count, or the wrong number of words is an error.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidBitArray`] when `bits` is negative or `elems` does not
    /// have length `(bits + 63) / 64`.
    pub fn try_from_proto(
        proto: Option<&eld_tendermint_proto::libs::bits::BitArray>,
    ) -> Result<Option<Self>, Error> {
        let Some(proto) = proto else {
            return Ok(None);
        };
        if proto.bits < 0 {
            return Err(Error::InvalidBitArray {
                detail: format!("negative bit count {}", proto.bits),
            });
        }
        if proto.bits == 0 {
            if proto.elems.is_empty() {
                return Ok(None);
            }
            return Err(Error::InvalidBitArray {
                detail: format!("expected 0 words for 0 bits, got {}", proto.elems.len()),
            });
        }
        let Some(len) = word_len(proto.bits) else {
            return Err(Error::InvalidBitArray {
                detail: format!("bit count {} overflows", proto.bits),
            });
        };
        if proto.elems.len() != len {
            return Err(Error::InvalidBitArray {
                detail: format!(
                    "expected {len} words for {} bits, got {}",
                    proto.bits,
                    proto.elems.len()
                ),
            });
        }
        Ok(Some(Self {
            bits: proto.bits,
            elems: proto.elems.clone(),
        }))
    }

    fn copy_bits(&self, bits: i64) -> Self {
        let len = word_len(bits).expect("bit count is positive and fits");
        let mut elems = vec![0; len];
        let n = len.min(self.elems.len());
        elems[..n].copy_from_slice(&self.elems[..n]);
        Self { bits, elems }
    }

    fn pattern(&self) -> String {
        let mut pattern = String::new();
        for i in 0..self.bits {
            pattern.push(if self.get_index(i) { 'x' } else { '_' });
        }
        pattern
    }

    fn from_pattern(pattern: &str) -> Result<Self, Error> {
        if pattern.is_empty() || !pattern.bytes().all(|byte| byte == b'x' || byte == b'_') {
            return Err(Error::InvalidBitArray {
                detail: format!(r#"json must match "([_x]*)", got {pattern:?}"#),
            });
        }
        let bits = i64::try_from(pattern.len()).map_err(|_| Error::InvalidBitArray {
            detail: format!("pattern length {} does not fit", pattern.len()),
        })?;
        let mut bit_array = Self::new(bits).ok_or_else(|| Error::InvalidBitArray {
            detail: format!("bit count {bits} is invalid"),
        })?;
        for (i, byte) in pattern.bytes().enumerate() {
            if byte == b'x' {
                bit_array.set_index(i64::try_from(i).expect("index fits"), true);
            }
        }
        Ok(bit_array)
    }
}

impl std::fmt::Display for BitArray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.string_indented(""))
    }
}

impl Serialize for BitArray {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.pattern())
    }
}

impl<'de> Deserialize<'de> for BitArray {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let pattern = String::deserialize(deserializer)?;
        Self::from_pattern(&pattern).map_err(serde::de::Error::custom)
    }
}

/// Word count `(bits + 63) / 64`. `None` when `bits <= 0` or the count does not fit.
fn word_len(bits: i64) -> Option<usize> {
    if bits <= 0 {
        return None;
    }
    let sum = bits.checked_add(63)?;
    usize::try_from(sum / 64).ok()
}

fn in_range(bits: i64, index: i64) -> Option<usize> {
    if index < 0 || index >= bits {
        None
    } else {
        usize::try_from(index).ok()
    }
}
