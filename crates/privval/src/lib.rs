//! `privval.FilePV` from Tendermint 0.34, and a dialer for a remote signer.
//!
//! Loads `config/priv_validator_key.json` and `data/priv_validator_state.json`.
//! Those are the paths `Config::priv_validator_key_file` and
//! `Config::priv_validator_state_file` return. This crate takes the paths
//! directly so it does not depend on the config loader. When
//! `priv_validator_laddr` is set, [`RemoteSigner`] dials that address instead.
//!
//! Writes are atomic: the bytes go to a temp file in the same directory, the file
//! is `fsync`ed, then renamed over the destination. Mode is `0600` on Unix.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use eld_tendermint_crypto::{
    Address, PrivKey, PubKey, marshal_priv_key, marshal_pub_key, unmarshal_priv_key,
};
use eld_tendermint_proto::types::{CanonicalProposal, CanonicalVote};
use eld_tendermint_types::{Proposal, SignedMsgType, Time, Vote};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod error;
mod remote;

pub use error::Error;
pub use remote::{RemoteSigner, read_delimited, sign_vote_message, write_delimited};

/// `stepNone`. Distinguishes an initial state that has never signed.
pub const STEP_NONE: i8 = 0;
/// `stepPropose`.
pub const STEP_PROPOSE: i8 = 1;
/// `stepPrevote`.
pub const STEP_PREVOTE: i8 = 2;
/// `stepPrecommit`.
pub const STEP_PRECOMMIT: i8 = 3;

/// `privval.FilePVKey`. JSON uses Amino objects for the keys and uppercase hex
/// for the address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePVKey {
    pub address: Address,
    pub pub_key: PubKey,
    pub priv_key: PrivKey,
}

/// `privval.FilePVLastSignState`.
///
/// JSON names are `height`, `round`, `step`, `signature`, and `signbytes`.
/// `height` is a decimal string, matching Tendermint's JSON int64 encoding.
/// Empty signature and sign bytes are omitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilePVLastSignState {
    pub height: i64,
    pub round: i32,
    pub step: i8,
    pub signature: Option<Vec<u8>>,
    pub sign_bytes: Option<Vec<u8>>,
}

/// What consensus asks a validator key to sign.
///
/// [`FilePV`] reads the local key. [`RemoteSigner`] dials `priv_validator_laddr`.
pub trait PrivValidator: Send {
    /// `GetPubKey`.
    fn get_pub_key(&self) -> PubKey;

    /// `SignVote`.
    ///
    /// # Errors
    ///
    /// Returns a regression, [`Error::ConflictingData`], or a socket error.
    fn sign_vote(&mut self, chain_id: &str, vote: &mut Vote) -> Result<(), Error>;

    /// `SignProposal`.
    ///
    /// # Errors
    ///
    /// Returns the same classes of error as [`Self::sign_vote`].
    fn sign_proposal(&mut self, chain_id: &str, proposal: &mut Proposal) -> Result<(), Error>;

    /// Last height, round, step, signature, and sign bytes this signer accepted.
    fn last_sign_state(&self) -> &FilePVLastSignState;
}

/// `privval.FilePV`.
#[derive(Clone, Debug)]
pub struct FilePV {
    pub key: FilePVKey,
    pub last_sign_state: FilePVLastSignState,
    key_path: PathBuf,
    state_path: PathBuf,
}

impl FilePV {
    /// `LoadFilePV`.
    ///
    /// The public key and address are taken from the private key, as Go does
    /// after unmarshalling.
    ///
    /// # Errors
    ///
    /// Returns an error when either file is missing or the JSON is not a key or
    /// last-sign-state document.
    pub fn load(
        key_path: impl Into<PathBuf>,
        state_path: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let key_path = key_path.into();
        let state_path = state_path.into();
        let key_bytes = fs::read(&key_path).map_err(|err| Error::Io {
            path: key_path.clone(),
            message: err.to_string(),
        })?;
        let key = FilePVKey::from_json(&key_bytes)?;
        let state_bytes = fs::read(&state_path).map_err(|err| Error::Io {
            path: state_path.clone(),
            message: err.to_string(),
        })?;
        let last_sign_state = FilePVLastSignState::from_json(&state_bytes)?;
        Ok(Self {
            key,
            last_sign_state,
            key_path,
            state_path,
        })
    }

    /// `LoadOrGenFilePV`. Generates an Ed25519 key when the key file is absent,
    /// then writes both files.
    ///
    /// # Errors
    ///
    /// Returns an error when the key file exists but cannot be loaded, or when
    /// saving a newly generated validator fails.
    pub fn load_or_gen_file_pv(
        key_path: impl Into<PathBuf>,
        state_path: impl Into<PathBuf>,
    ) -> Result<Self, Error> {
        let key_path = key_path.into();
        let state_path = state_path.into();
        if key_path.is_file() {
            Self::load(key_path, state_path)
        } else {
            let pv = Self::generate(key_path, state_path);
            pv.save()?;
            Ok(pv)
        }
    }

    /// `GenFilePV`. Does not write the files.
    #[must_use]
    pub fn generate(key_path: impl Into<PathBuf>, state_path: impl Into<PathBuf>) -> Self {
        let priv_key = PrivKey::generate();
        let pub_key = priv_key
            .public_key()
            .expect("generated key includes its public key");
        Self {
            key: FilePVKey {
                address: pub_key.address(),
                pub_key,
                priv_key,
            },
            last_sign_state: FilePVLastSignState {
                height: 0,
                round: 0,
                step: STEP_NONE,
                signature: None,
                sign_bytes: None,
            },
            key_path: key_path.into(),
            state_path: state_path.into(),
        }
    }

    /// `FilePV.Save`. Writes the key file and the state file.
    ///
    /// # Errors
    ///
    /// Returns an error when either atomic write fails.
    pub fn save(&self) -> Result<(), Error> {
        self.save_key()?;
        self.save_state()
    }

    /// `FilePVKey.Save`.
    ///
    /// # Errors
    ///
    /// Returns an error when the atomic write fails.
    pub fn save_key(&self) -> Result<(), Error> {
        write_atomic(&self.key_path, &self.key.to_json())
    }

    /// `FilePVLastSignState.Save`.
    ///
    /// # Errors
    ///
    /// Returns an error when the atomic write fails.
    pub fn save_state(&self) -> Result<(), Error> {
        write_atomic(&self.state_path, &self.last_sign_state.to_json())
    }

    /// `FilePV.GetPubKey`.
    #[must_use]
    pub fn get_pub_key(&self) -> PubKey {
        self.key.pub_key
    }

    /// `FilePV.SignVote`.
    ///
    /// Same height, round, and step with the same sign bytes returns the stored
    /// signature. The same HRS with different sign bytes is a double-sign error,
    /// unless the canonical votes differ only by timestamp, in which case the
    /// previous timestamp and signature are restored. A lower height, round, or
    /// step is a regression. A new HRS is signed and the state file is written
    /// before this returns.
    ///
    /// # Errors
    ///
    /// Returns a regression error, [`Error::ConflictingData`], or an I/O error
    /// from persisting the new state.
    pub fn sign_vote(&mut self, chain_id: &str, vote: &mut Vote) -> Result<(), Error> {
        let step = vote_step(vote.vote_type)?;
        self.sign_bytes(
            vote.height,
            vote.round,
            step,
            vote.sign_bytes(chain_id),
            |signature, timestamp| {
                if let Some(timestamp) = timestamp {
                    vote.timestamp = timestamp;
                }
                vote.signature = signature;
            },
        )
    }

    /// `FilePV.SignProposal`. Uses [`STEP_PROPOSE`].
    ///
    /// # Errors
    ///
    /// Returns the same classes of error as [`Self::sign_vote`].
    pub fn sign_proposal(&mut self, chain_id: &str, proposal: &mut Proposal) -> Result<(), Error> {
        self.sign_bytes(
            proposal.height,
            proposal.round,
            STEP_PROPOSE,
            proposal.sign_bytes(chain_id),
            |signature, timestamp| {
                if let Some(timestamp) = timestamp {
                    proposal.timestamp = timestamp;
                }
                proposal.signature = signature;
            },
        )
    }

    fn sign_bytes(
        &mut self,
        height: i64,
        round: i32,
        step: i8,
        sign_bytes: Vec<u8>,
        apply: impl FnOnce(Vec<u8>, Option<Time>),
    ) -> Result<(), Error> {
        // Compare (height, round, step) with the last signed state before signing.
        let same_hrs = self.last_sign_state.check_hrs(height, round, step)?;
        if same_hrs {
            // Same HRS + same sign bytes => replay the stored signature.
            // Same HRS + different sign bytes => double sign, unless only the
            // timestamp differs, in which case the previous timestamp is kept.
            let last_bytes = self
                .last_sign_state
                .sign_bytes
                .clone()
                .ok_or(Error::NoSignBytes)?;
            let signature = self
                .last_sign_state
                .signature
                .clone()
                .ok_or(Error::MissingLastSignature)?;
            if sign_bytes == last_bytes {
                apply(signature, None);
                return Ok(());
            }
            let timestamp = if step == STEP_PROPOSE {
                proposals_only_differ_by_timestamp(&last_bytes, &sign_bytes)?
            } else {
                votes_only_differ_by_timestamp(&last_bytes, &sign_bytes)?
            };
            if let Some(timestamp) = timestamp {
                apply(signature, Some(timestamp));
                return Ok(());
            }
            return Err(Error::ConflictingData);
        }

        let signature = self.key.priv_key.sign(&sign_bytes).map_err(Error::Crypto)?;
        self.last_sign_state.height = height;
        self.last_sign_state.round = round;
        self.last_sign_state.step = step;
        self.last_sign_state.signature = Some(signature.to_vec());
        self.last_sign_state.sign_bytes = Some(sign_bytes);
        // Persist the new state before returning the signature.
        self.save_state()?;
        apply(signature.to_vec(), None);
        Ok(())
    }
}

impl PrivValidator for FilePV {
    fn get_pub_key(&self) -> PubKey {
        FilePV::get_pub_key(self)
    }

    fn sign_vote(&mut self, chain_id: &str, vote: &mut Vote) -> Result<(), Error> {
        FilePV::sign_vote(self, chain_id, vote)
    }

    fn sign_proposal(&mut self, chain_id: &str, proposal: &mut Proposal) -> Result<(), Error> {
        FilePV::sign_proposal(self, chain_id, proposal)
    }

    fn last_sign_state(&self) -> &FilePVLastSignState {
        &self.last_sign_state
    }
}

impl PrivValidator for Box<dyn PrivValidator> {
    fn get_pub_key(&self) -> PubKey {
        self.as_ref().get_pub_key()
    }

    fn sign_vote(&mut self, chain_id: &str, vote: &mut Vote) -> Result<(), Error> {
        self.as_mut().sign_vote(chain_id, vote)
    }

    fn sign_proposal(&mut self, chain_id: &str, proposal: &mut Proposal) -> Result<(), Error> {
        self.as_mut().sign_proposal(chain_id, proposal)
    }

    fn last_sign_state(&self) -> &FilePVLastSignState {
        self.as_ref().last_sign_state()
    }
}

impl FilePVKey {
    fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let file: KeyFile =
            serde_json::from_slice(bytes).map_err(|err| Error::Json(err.to_string()))?;
        let _address = file.address;
        let priv_json =
            serde_json::to_string(&file.priv_key).map_err(|err| Error::Json(err.to_string()))?;
        let priv_key = unmarshal_priv_key(&priv_json).map_err(Error::Key)?;
        let pub_key = priv_key.public_key().map_err(Error::Key)?;
        Ok(Self {
            address: pub_key.address(),
            pub_key,
            priv_key,
        })
    }

    fn to_json(&self) -> Vec<u8> {
        let pub_key: Value = serde_json::from_str(&marshal_pub_key(&self.pub_key))
            .expect("amino public key json is an object");
        let priv_key: Value = serde_json::from_str(&marshal_priv_key(&self.priv_key))
            .expect("amino private key json is an object");
        let file = KeyFile {
            address: hex::encode_upper(self.address),
            pub_key,
            priv_key,
        };
        let mut bytes = serde_json::to_vec_pretty(&file).expect("key json");
        bytes.push(b'\n');
        bytes
    }
}

impl FilePVLastSignState {
    /// Returns `Ok(true)` when this HRS was already signed and the stored
    /// signature can be reused. Returns `Ok(false)` when the HRS is newer.
    ///
    /// # Errors
    ///
    /// Returns an error on height, round, or step regression, or when the HRS
    /// matches but no sign bytes were stored.
    fn check_hrs(&self, height: i64, round: i32, step: i8) -> Result<bool, Error> {
        if self.height > height {
            return Err(Error::HeightRegression {
                got: height,
                last: self.height,
            });
        }
        if self.height == height {
            if self.round > round {
                return Err(Error::RoundRegression {
                    height,
                    got: round,
                    last: self.round,
                });
            }
            if self.round == round {
                return match self.step.cmp(&step) {
                    std::cmp::Ordering::Greater => Err(Error::StepRegression {
                        height,
                        round,
                        got: step,
                        last: self.step,
                    }),
                    std::cmp::Ordering::Equal => {
                        if self
                            .sign_bytes
                            .as_ref()
                            .is_some_and(|bytes| !bytes.is_empty())
                        {
                            match &self.signature {
                                Some(signature) if !signature.is_empty() => Ok(true),
                                _ => Err(Error::MissingLastSignature),
                            }
                        } else {
                            Err(Error::NoSignBytes)
                        }
                    }
                    std::cmp::Ordering::Less => Ok(false),
                };
            }
        }
        Ok(false)
    }

    fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let file: StateFile =
            serde_json::from_slice(bytes).map_err(|err| Error::Json(err.to_string()))?;
        Ok(Self {
            height: file.height,
            round: file.round,
            step: file.step,
            signature: file.signature.filter(|bytes| !bytes.is_empty()),
            sign_bytes: file.signbytes.filter(|bytes| !bytes.is_empty()),
        })
    }

    fn to_json(&self) -> Vec<u8> {
        let file = StateFile {
            height: self.height,
            round: self.round,
            step: self.step,
            signature: self.signature.clone().filter(|bytes| !bytes.is_empty()),
            signbytes: self.sign_bytes.clone().filter(|bytes| !bytes.is_empty()),
        };
        let mut bytes = serde_json::to_vec_pretty(&file).expect("state json");
        bytes.push(b'\n');
        bytes
    }
}

pub(crate) fn vote_step(vote_type: SignedMsgType) -> Result<i8, Error> {
    match vote_type {
        SignedMsgType::Prevote => Ok(STEP_PREVOTE),
        SignedMsgType::Precommit => Ok(STEP_PRECOMMIT),
        _ => Err(Error::UnknownVoteType),
    }
}

/// Returns the timestamp from the last sign bytes when that is the only difference.
fn votes_only_differ_by_timestamp(last: &[u8], new: &[u8]) -> Result<Option<Time>, Error> {
    let mut last_vote =
        CanonicalVote::decode_length_delimited(last).map_err(|_| Error::CorruptSignBytes)?;
    let mut new_vote =
        CanonicalVote::decode_length_delimited(new).map_err(|_| Error::CorruptSignBytes)?;
    let last_time = Time::from_prost(last_vote.timestamp.as_ref());
    last_vote.timestamp = None;
    new_vote.timestamp = None;
    if last_vote == new_vote {
        Ok(Some(last_time))
    } else {
        Ok(None)
    }
}

fn proposals_only_differ_by_timestamp(last: &[u8], new: &[u8]) -> Result<Option<Time>, Error> {
    let mut last_proposal =
        CanonicalProposal::decode_length_delimited(last).map_err(|_| Error::CorruptSignBytes)?;
    let mut new_proposal =
        CanonicalProposal::decode_length_delimited(new).map_err(|_| Error::CorruptSignBytes)?;
    let last_time = Time::from_prost(last_proposal.timestamp.as_ref());
    last_proposal.timestamp = None;
    new_proposal.timestamp = None;
    if last_proposal == new_proposal {
        Ok(Some(last_time))
    } else {
        Ok(None)
    }
}

/// Writes `bytes` to a sibling temp file, `fsync`s it, and renames it onto `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("privval");
    let tmp_path = parent.join(format!(".{file_name}.tmp"));
    let io_err = |err: std::io::Error| Error::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    };
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp_path).map_err(io_err)?;
        file.write_all(bytes).map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
    }
    fs::rename(&tmp_path, path).map_err(io_err)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    address: String,
    pub_key: Value,
    priv_key: Value,
}

#[derive(Serialize, Deserialize)]
struct StateFile {
    #[serde(serialize_with = "ser_i64", deserialize_with = "de_i64")]
    height: i64,
    round: i32,
    step: i8,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "hex_bytes")]
    signature: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "hex_bytes")]
    signbytes: Option<Vec<u8>>,
}

fn ser_i64<S: serde::Serializer>(value: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

fn de_i64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse().map_err(serde::de::Error::custom)
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => serializer.serialize_str(&hex::encode_upper(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let text = Option::<String>::deserialize(deserializer)?;
        match text {
            Some(text) if !text.is_empty() => hex::decode(&text)
                .map(Some)
                .map_err(serde::de::Error::custom),
            _ => Ok(None),
        }
    }
}
