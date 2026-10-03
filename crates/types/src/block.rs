//! `types.Block` and `MakeBlock`.
//!
//! There is no mutex. Go locks `ValidateBasic`, `Hash`, and `MakePartSet`.

use prost::Message;

use crate::evidence::EvidenceList;
use crate::{
    BlockId, ChainId, Commit, ConsensusVersion, Error, Hash, Header, PartSet, Time, Tx, Txs,
};

/// `types.Block`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub data: Txs,
    pub evidence: EvidenceList,
    pub last_commit: Option<Commit>,
}

impl Block {
    /// `MakeBlock`.
    ///
    /// Fills `data_hash`, `evidence_hash`, and `last_commit_hash`. A nil last commit
    /// stores an empty `last_commit_hash` (Go's nil hash), not the empty-tree hash.
    /// Version is block protocol 11. Time, last block id, proposer address, and the
    /// validator and consensus hashes stay zero for the caller to set.
    #[must_use]
    pub fn make_block(
        height: i64,
        txs: Txs,
        last_commit: Option<Commit>,
        evidence: EvidenceList,
    ) -> Self {
        let last_commit_hash = last_commit
            .as_ref()
            .map(|commit| commit.hash().as_bytes().to_vec())
            .unwrap_or_default();
        Self {
            header: Header {
                version: ConsensusVersion::default(),
                chain_id: ChainId::new(""),
                height,
                time: Time::GO_ZERO,
                last_block_id: BlockId::default(),
                last_commit_hash,
                data_hash: txs.hash().as_bytes().to_vec(),
                validators_hash: Vec::new(),
                next_validators_hash: Vec::new(),
                consensus_hash: Vec::new(),
                app_hash: Vec::new(),
                last_results_hash: Vec::new(),
                evidence_hash: evidence.hash().as_bytes().to_vec(),
                proposer_address: Vec::new(),
            },
            data: txs,
            evidence,
            last_commit,
        }
    }

    /// `Block.ValidateBasic`.
    ///
    /// A nil last commit is an error at every height, including height 1.
    ///
    /// # Errors
    ///
    /// Returns the header error, [`Error::NilLastCommit`], a commit error, an evidence
    /// error, or a mismatched `last_commit_hash`, `data_hash`, or `evidence_hash`.
    pub fn validate_basic(&self) -> Result<(), Error> {
        self.header.validate_basic()?;
        let Some(commit) = &self.last_commit else {
            return Err(Error::NilLastCommit);
        };
        commit.validate_basic()?;
        self.evidence.validate_basic()?;
        if self.header.last_commit_hash.as_slice() != commit.hash().as_bytes().as_slice() {
            return Err(Error::WrongLastCommitHash);
        }
        if self.header.data_hash.as_slice() != self.data.hash().as_bytes().as_slice() {
            return Err(Error::WrongDataHash);
        }
        if self.header.evidence_hash.as_slice() != self.evidence.hash().as_bytes().as_slice() {
            return Err(Error::WrongEvidenceHash);
        }
        Ok(())
    }

    /// `Block.Hash`: the header hash.
    ///
    /// `None` when `last_commit` is missing, and when the header hash is missing
    /// (empty `validators_hash`).
    #[must_use]
    pub fn hash(&self) -> Option<Hash> {
        self.last_commit.as_ref()?;
        self.header.hash()
    }

    /// `Block.ToProto`. A missing last commit omits the field.
    /// Each evidence item is the `Evidence` oneof, duplicate vote or light-client attack.
    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::Block {
        eld_tendermint_proto::types::Block {
            header: Some(self.header.to_proto()),
            data: Some(eld_tendermint_proto::types::Data {
                txs: self
                    .data
                    .as_slice()
                    .iter()
                    .map(|tx| tx.as_bytes().to_vec())
                    .collect(),
            }),
            evidence: Some(self.evidence.to_proto()),
            last_commit: self.last_commit.as_ref().map(Commit::to_proto),
        }
    }

    /// `BlockFromProto`.
    ///
    /// # Errors
    ///
    /// Returns a header, data, commit, or evidence error, then
    /// the same errors as [`Self::validate_basic`].
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::Block) -> Result<Self, Error> {
        let Some(header) = proto.header.as_ref() else {
            return Err(Error::MissingHeader);
        };
        let evidence = match &proto.evidence {
            Some(list) => EvidenceList::try_from_proto(list)?,
            None => EvidenceList::default(),
        };
        let txs = proto
            .data
            .as_ref()
            .map(|data| Txs::new(data.txs.iter().map(|tx| Tx::new(tx.clone())).collect()))
            .unwrap_or_default();
        let last_commit = match &proto.last_commit {
            Some(commit) => Some(Commit::try_from_proto(commit)?),
            None => None,
        };
        let block = Self {
            header: Header::try_from_proto(header)?,
            data: txs,
            evidence,
            last_commit,
        };
        block.validate_basic()?;
        Ok(block)
    }

    /// `Block.MakePartSet`.
    ///
    /// Marshals the protobuf block and splits it with [`PartSet::from_data`].
    /// Callers use [`crate::BLOCK_PART_SIZE_BYTES`] (65536) unless a test passes another size.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ZeroPartSize`] when `part_size` is 0. Go panics on that
    /// contract violation. Also returns [`Error::TooManyParts`] from `PartSet`.
    pub fn make_part_set(&self, part_size: u32) -> Result<PartSet, Error> {
        let bytes = self.to_proto().encode_to_vec();
        PartSet::from_data(&bytes, part_size)
    }
}
