//! `types.SignedHeader` and `types.LightBlock`.
//!
//! These are the pieces a light-client attack carries. Signature checks stay on
//! [`crate::ValidatorSet::verify_commit_light`].

use crate::{Commit, Error, Header, ValidatorSet};

/// `types.SignedHeader`. A header and the commit that signs it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedHeader {
    pub header: Header,
    pub commit: Commit,
}

impl SignedHeader {
    /// `SignedHeader.ValidateBasic`. Does not check signatures.
    ///
    /// # Errors
    ///
    /// Returns a header or commit error, or a chain, height, or block-id mismatch.
    pub fn validate_basic(&self, chain_id: &str) -> Result<(), Error> {
        self.header.validate_basic()?;
        self.commit.validate_basic()?;
        if self.header.chain_id.as_str() != chain_id {
            return Err(Error::ChainIdMismatch);
        }
        if self.commit.height != self.header.height {
            return Err(Error::HeaderCommitMismatch);
        }
        let Some(hash) = self.header.hash() else {
            return Err(Error::MissingHeader);
        };
        if hash.as_bytes().as_slice() != self.commit.block_id.hash.as_slice() {
            return Err(Error::CommitSignsWrongBlock);
        }
        Ok(())
    }

    #[must_use]
    pub fn to_proto(&self) -> eld_tendermint_proto::types::SignedHeader {
        eld_tendermint_proto::types::SignedHeader {
            header: Some(self.header.to_proto()),
            commit: Some(self.commit.to_proto()),
        }
    }

    /// # Errors
    ///
    /// Returns [`Error::MissingHeader`], a header error, or a commit error, then
    /// [`Self::validate_basic`] against the header's own chain id.
    pub fn try_from_proto(
        proto: &eld_tendermint_proto::types::SignedHeader,
    ) -> Result<Self, Error> {
        let Some(header) = proto.header.as_ref() else {
            return Err(Error::MissingHeader);
        };
        let Some(commit) = proto.commit.as_ref() else {
            return Err(Error::NilLastCommit);
        };
        let signed = Self {
            header: Header::try_from_proto(header)?,
            commit: Commit::try_from_proto(commit)?,
        };
        signed.validate_basic(signed.header.chain_id.as_str())?;
        Ok(signed)
    }
}

/// `types.LightBlock`. A signed header plus the validator set that signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightBlock {
    pub signed_header: SignedHeader,
    pub validator_set: ValidatorSet,
}

impl LightBlock {
    /// `LightBlock.ValidateBasic`. Does not check signatures.
    ///
    /// # Errors
    ///
    /// Returns a signed-header or validator-set error, or [`Error::ValidatorHashMismatch`].
    pub fn validate_basic(&self, chain_id: &str) -> Result<(), Error> {
        self.signed_header.validate_basic(chain_id)?;
        self.validator_set.validate_basic()?;
        let hash = self.validator_set.hash()?;
        if hash.as_bytes().as_slice() != self.signed_header.header.validators_hash.as_slice() {
            return Err(Error::ValidatorHashMismatch);
        }
        Ok(())
    }

    /// # Errors
    ///
    /// Returns a validator proto error.
    pub fn to_proto(&self) -> Result<eld_tendermint_proto::types::LightBlock, Error> {
        Ok(eld_tendermint_proto::types::LightBlock {
            signed_header: Some(self.signed_header.to_proto()),
            validator_set: Some(self.validator_set.to_proto()?),
        })
    }

    /// # Errors
    ///
    /// Returns [`Error::MissingSignedHeader`], [`Error::MissingLightValidatorSet`], or a
    /// nested decode error, then [`Self::validate_basic`].
    pub fn try_from_proto(proto: &eld_tendermint_proto::types::LightBlock) -> Result<Self, Error> {
        let Some(signed_header) = proto.signed_header.as_ref() else {
            return Err(Error::MissingSignedHeader);
        };
        let Some(validator_set) = proto.validator_set.as_ref() else {
            return Err(Error::MissingLightValidatorSet);
        };
        let block = Self {
            signed_header: SignedHeader::try_from_proto(signed_header)?,
            validator_set: ValidatorSet::try_from_proto(validator_set)?,
        };
        block.validate_basic(block.signed_header.header.chain_id.as_str())?;
        Ok(block)
    }
}
