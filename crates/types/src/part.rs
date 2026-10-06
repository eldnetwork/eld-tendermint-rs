//! `types.Part` and `types.PartSet`.
//!
//! Go locks a mutex around `AddPart`, `GetPart`, and `BitArray`. This type does not.

use eld_tendermint_crypto::{Proof, proofs_from_byte_slices};

use crate::ensured::Ensured;
use crate::{BLOCK_PART_SIZE_BYTES, BitArray, Error, PartSetHeader};

/// `types.Part`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub index: u32,
    pub bytes: Vec<u8>,
    pub proof: Proof,
}

impl Part {
    /// `Part.ValidateBasic`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PartTooBig`] when `bytes` is longer than
    /// [`BLOCK_PART_SIZE_BYTES`], or [`Error::Proof`] when the proof fails
    /// `ValidateBasic`.
    pub fn validate_basic(&self) -> Result<(), Error> {
        if self.bytes.len() > usize::try_from(BLOCK_PART_SIZE_BYTES).ensured("part size fits") {
            return Err(Error::PartTooBig {
                len: self.bytes.len(),
            });
        }
        self.proof.validate_basic().map_err(Error::Proof)
    }

    /// `Part.ToProto`.
    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Part {
        eld_tendermint_proto::types::Part {
            index: self.index,
            bytes: self.bytes.clone(),
            proof: Some(self.proof.to_proto()),
        }
    }

    /// `PartFromProto`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingPart`] when `proto` is missing, [`Error::Proof`]
    /// when the proof is missing or invalid, or the same errors as
    /// [`Self::validate_basic`].
    pub fn try_from_proto(
        proto: Option<&eld_tendermint_proto::types::Part>,
    ) -> Result<Self, Error> {
        let Some(proto) = proto else {
            return Err(Error::MissingPart);
        };
        let proof = Proof::try_from_proto(proto.proof.as_ref()).map_err(Error::Proof)?;
        let part = Self {
            index: proto.index,
            bytes: proto.bytes.clone(),
            proof,
        };
        part.validate_basic()?;
        Ok(part)
    }
}

/// `types.PartSet`. Parts are stored in index order. Absent parts are `None`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartSet {
    total: u32,
    hash: Vec<u8>,
    parts: Vec<Option<Part>>,
    parts_bit_array: Option<BitArray>,
    count: u32,
    byte_size: i64,
}

impl PartSet {
    /// `NewPartSetFromData`.
    ///
    /// Splits `data` into `part_size` chunks, the last chunk shorter, and stores
    /// a merkle proof for each chunk. An empty blob has no parts and the empty-tree
    /// hash. `BitArray::new(0)` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ZeroPartSize`] when `part_size` is 0, or
    /// [`Error::TooManyParts`] when the chunk count does not fit in `u32`.
    pub fn from_data(data: &[u8], part_size: u32) -> Result<Self, Error> {
        if part_size == 0 {
            return Err(Error::ZeroPartSize);
        }
        let part_size = usize::try_from(part_size).ensured("part size fits");
        let total_usize = data.len().div_ceil(part_size);
        let total =
            u32::try_from(total_usize).map_err(|_| Error::TooManyParts { count: total_usize })?;
        let chunks: Vec<&[u8]> = (0..total_usize)
            .map(|i| {
                let start = i * part_size;
                let end = (start + part_size).min(data.len());
                &data[start..end]
            })
            .collect();
        let (root, proofs) = proofs_from_byte_slices(&chunks);
        let mut bit_array = BitArray::new(i64::from(total));
        let mut parts = Vec::with_capacity(chunks.len());
        for (index, (bytes, proof)) in chunks.into_iter().zip(proofs).enumerate() {
            if let Some(bits) = bit_array.as_mut() {
                bits.set_index(i64::try_from(index).ensured("part index fits"), true);
            }
            parts.push(Some(Part {
                index: u32::try_from(index).ensured("part index fits"),
                bytes: bytes.to_vec(),
                proof,
            }));
        }
        Ok(Self {
            total,
            hash: root.to_vec(),
            parts,
            parts_bit_array: bit_array,
            count: total,
            byte_size: i64::try_from(data.len()).ensured("data length fits in i64"),
        })
    }

    /// `NewPartSetFromHeader`. No parts are present yet.
    #[must_use]
    pub fn from_header(header: PartSetHeader) -> Self {
        let total = header.total;
        Self {
            total,
            hash: header.hash,
            parts: vec![None; usize::try_from(total).ensured("part count fits")],
            parts_bit_array: BitArray::new(i64::from(total)),
            count: 0,
            byte_size: 0,
        }
    }

    /// `PartSet.Header`.
    #[must_use]
    pub fn header(&self) -> PartSetHeader {
        PartSetHeader {
            total: self.total,
            hash: self.hash.clone(),
        }
    }

    /// `PartSet.HasHeader`.
    #[must_use]
    pub fn has_header(&self, header: &PartSetHeader) -> bool {
        self.total == header.total && self.hash == header.hash
    }

    /// `PartSet.Hash`. Merkle root of the part blobs.
    #[must_use]
    pub fn hash(&self) -> &[u8] {
        &self.hash
    }

    /// `PartSet.Total`.
    #[must_use]
    pub const fn total(&self) -> u32 {
        self.total
    }

    /// `PartSet.Count`.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// `PartSet.ByteSize`.
    #[must_use]
    pub const fn byte_size(&self) -> i64 {
        self.byte_size
    }

    /// `PartSet.IsComplete`.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.count == self.total
    }

    /// `PartSet.BitArray`. A copy, so later `add_part` calls do not change it.
    /// `None` when `total` is 0.
    #[must_use]
    pub fn bit_array(&self) -> Option<BitArray> {
        self.parts_bit_array.as_ref().map(BitArray::copy)
    }

    /// `PartSet.GetPart`. `None` when the index is outside the set or not yet added.
    #[must_use]
    pub fn get_part(&self, index: u32) -> Option<&Part> {
        self.parts
            .get(usize::try_from(index).ok()?)
            .and_then(Option::as_ref)
    }

    /// `PartSet.AddPart`.
    ///
    /// `Ok(false)` when that index is already present. The proof is checked
    /// against [`Self::hash`] and the part bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnexpectedPartIndex`] when `part.index >= total`, or
    /// [`Error::InvalidPartProof`] when the proof does not verify.
    pub fn add_part(&mut self, part: Part) -> Result<bool, Error> {
        if part.index >= self.total {
            return Err(Error::UnexpectedPartIndex {
                index: part.index,
                total: self.total,
            });
        }
        let index = usize::try_from(part.index).ensured("part index fits");
        if self.parts[index].is_some() {
            return Ok(false);
        }
        if part.proof.verify(&self.hash, &part.bytes).is_err() {
            return Err(Error::InvalidPartProof);
        }
        if let Some(bits) = self.parts_bit_array.as_mut() {
            bits.set_index(i64::from(part.index), true);
        }
        self.byte_size += i64::try_from(part.bytes.len()).ensured("part length fits in i64");
        self.count += 1;
        self.parts[index] = Some(part);
        Ok(true)
    }

    /// Bytes of every part, in order.
    ///
    /// An incomplete set returns [`Error::IncompletePartSet`]. Go panics in
    /// `GetReader` in that case. A complete set with `total == 0` is empty.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IncompletePartSet`] when [`Self::is_complete`] is false.
    pub fn read_bytes(&self) -> Result<Vec<u8>, Error> {
        if !self.is_complete() {
            return Err(Error::IncompletePartSet);
        }
        let mut out = Vec::with_capacity(usize::try_from(self.byte_size).unwrap_or(0));
        for part in &self.parts {
            let part = part.as_ref().ok_or(Error::IncompletePartSet)?;
            out.extend_from_slice(&part.bytes);
        }
        Ok(out)
    }
}
