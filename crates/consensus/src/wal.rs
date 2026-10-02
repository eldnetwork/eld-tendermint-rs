//! Consensus WAL. One append-only file of CRC32C-framed `TimedWALMessage` records.
//!
//! Format, from `consensus/wal.go`: 4-byte Castagnoli CRC, 4-byte big-endian length,
//! then the protobuf bytes. The CRC covers only the protobuf. There is no autofile
//! rotation and no flush ticker. `WriteSync` appends and `fsync`s before it returns.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use eld_tendermint_proto::consensus::message;
use eld_tendermint_proto::consensus::wal_message;
use eld_tendermint_proto::consensus::{
    self, BlockPart, EndHeight, MsgInfo, TimeoutInfo, WalMessage,
};
use eld_tendermint_types::{Part, Proposal, Time, Vote};
use prost::Message;

use crate::error::Error;

/// `maxMsgSize + 24` from `consensus/wal.go`.
const MAX_MSG_SIZE_BYTES: u32 = 1_048_576 + 24;

/// Append-only consensus WAL at `data/cs.wal/wal`.
pub struct Wal {
    path: PathBuf,
    file: File,
}

impl Wal {
    /// Opens `path`, creating parent directories. An empty file gets `EndHeight { 0 }`.
    ///
    /// # Errors
    ///
    /// Returns an I/O error, or a WAL error if the initial marker cannot be written.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Io(err.to_string()))?;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true).read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(&path)
            .map_err(|err| Error::Io(err.to_string()))?;
        let empty = file
            .metadata()
            .map_err(|err| Error::Io(err.to_string()))?
            .len()
            == 0;
        let mut wal = Self { path, file };
        if empty {
            wal.write_message(&end_height_message(Time::now(), 0))?;
        }
        Ok(wal)
    }

    /// Encodes `msg`, appends it, and `fsync`s before returning.
    ///
    /// # Errors
    ///
    /// Returns an I/O error, or [`Error::CorruptWal`] when the protobuf is larger
    /// than the Go maximum.
    pub fn write_message(
        &mut self,
        msg: &eld_tendermint_proto::consensus::TimedWalMessage,
    ) -> Result<(), Error> {
        let bytes = encode(msg)?;
        self.file
            .write_all(&bytes)
            .map_err(|err| Error::Io(err.to_string()))?;
        self.file
            .sync_all()
            .map_err(|err| Error::Io(err.to_string()))?;
        Ok(())
    }

    /// Every record in the file, in order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptWal`] on a torn or mismatched record.
    pub fn read_messages(
        &self,
    ) -> Result<Vec<eld_tendermint_proto::consensus::TimedWalMessage>, Error> {
        let mut file = File::open(&self.path).map_err(|err| Error::Io(err.to_string()))?;
        let mut out = Vec::new();
        loop {
            match decode_one(&mut file)? {
                None => return Ok(out),
                Some(msg) => out.push(msg),
            }
        }
    }

    /// Records after the first `EndHeight` for `height`. `None` when that marker is absent.
    /// The marker itself is not included.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptWal`] on a bad record.
    pub fn messages_after_end_height(
        &self,
        height: i64,
    ) -> Result<Option<Vec<eld_tendermint_proto::consensus::TimedWalMessage>>, Error> {
        let messages = self.read_messages()?;
        let mut found = false;
        let mut after = Vec::new();
        for msg in messages {
            if found {
                after.push(msg);
                continue;
            }
            if end_height_of(&msg) == Some(height) {
                found = true;
            }
        }
        if found { Ok(Some(after)) } else { Ok(None) }
    }
}

/// `MsgInfo` wrapping a proposal. `peer_id` is empty.
#[must_use]
pub fn proposal_message(
    time: Time,
    proposal: &Proposal,
) -> eld_tendermint_proto::consensus::TimedWalMessage {
    timed(
        time,
        wal_message::Sum::MsgInfo(MsgInfo {
            msg: Some(consensus::Message {
                sum: Some(message::Sum::Proposal(consensus::Proposal {
                    proposal: Some(proposal.to_proto()),
                })),
            }),
            peer_id: String::new(),
        }),
    )
}

/// `MsgInfo` wrapping a block part.
#[must_use]
pub fn part_message(
    time: Time,
    height: i64,
    round: i32,
    part: &Part,
) -> eld_tendermint_proto::consensus::TimedWalMessage {
    timed(
        time,
        wal_message::Sum::MsgInfo(MsgInfo {
            msg: Some(consensus::Message {
                sum: Some(message::Sum::BlockPart(BlockPart {
                    height,
                    round,
                    part: Some(part.to_proto()),
                })),
            }),
            peer_id: String::new(),
        }),
    )
}

/// `MsgInfo` wrapping a vote.
#[must_use]
pub fn vote_message(time: Time, vote: &Vote) -> eld_tendermint_proto::consensus::TimedWalMessage {
    timed(
        time,
        wal_message::Sum::MsgInfo(MsgInfo {
            msg: Some(consensus::Message {
                sum: Some(message::Sum::Vote(consensus::Vote {
                    vote: Some(vote.to_proto()),
                })),
            }),
            peer_id: String::new(),
        }),
    )
}

/// `TimeoutInfo`. `step` is the Go `RoundStepType` value.
#[must_use]
pub fn timeout_message(
    time: Time,
    duration_nanos: i64,
    height: i64,
    round: i32,
    step: u32,
) -> eld_tendermint_proto::consensus::TimedWalMessage {
    let seconds = duration_nanos.div_euclid(1_000_000_000);
    let nanos = i32::try_from(duration_nanos.rem_euclid(1_000_000_000)).unwrap_or(0);
    timed(
        time,
        wal_message::Sum::TimeoutInfo(TimeoutInfo {
            duration: Some(prost_types::Duration { seconds, nanos }),
            height,
            round,
            step,
        }),
    )
}

/// `EndHeight`.
#[must_use]
pub fn end_height_message(
    time: Time,
    height: i64,
) -> eld_tendermint_proto::consensus::TimedWalMessage {
    timed(time, wal_message::Sum::EndHeight(EndHeight { height }))
}

/// One replayable WAL body. `EventDataRoundState` is ignored.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum Replay {
    Proposal(Proposal),
    Part {
        height: i64,
        round: i32,
        part: Part,
    },
    Vote(Vote),
    Timeout {
        duration_nanos: i64,
        height: i64,
        round: i32,
        step: u32,
    },
    EndHeight(i64),
    Ignored,
}

/// # Errors
///
/// Returns [`Error::CorruptWal`] when a wrapped proposal, part, or vote will not decode.
pub fn replay_of(msg: &eld_tendermint_proto::consensus::TimedWalMessage) -> Result<Replay, Error> {
    let Some(body) = msg.msg.as_ref().and_then(|msg| msg.sum.as_ref()) else {
        return Err(Error::CorruptWal("empty WAL message".to_owned()));
    };
    match body {
        wal_message::Sum::EventDataRoundState(_) => Ok(Replay::Ignored),
        wal_message::Sum::EndHeight(end) => Ok(Replay::EndHeight(end.height)),
        wal_message::Sum::TimeoutInfo(info) => {
            let duration = info.duration.unwrap_or_default();
            let nanos = duration
                .seconds
                .saturating_mul(1_000_000_000)
                .saturating_add(i64::from(duration.nanos));
            Ok(Replay::Timeout {
                duration_nanos: nanos,
                height: info.height,
                round: info.round,
                step: info.step,
            })
        }
        wal_message::Sum::MsgInfo(info) => msg_info(info),
    }
}

fn msg_info(info: &MsgInfo) -> Result<Replay, Error> {
    let Some(sum) = info.msg.as_ref().and_then(|msg| msg.sum.as_ref()) else {
        return Err(Error::CorruptWal("empty msg info".to_owned()));
    };
    match sum {
        message::Sum::Proposal(proposal) => {
            let proto = proposal
                .proposal
                .as_ref()
                .ok_or_else(|| Error::CorruptWal("missing proposal".to_owned()))?;
            Proposal::try_from_proto(proto)
                .map(Replay::Proposal)
                .map_err(|err| Error::CorruptWal(err.to_string()))
        }
        message::Sum::BlockPart(part) => {
            let decoded = Part::try_from_proto(part.part.as_ref())
                .map_err(|err| Error::CorruptWal(err.to_string()))?;
            Ok(Replay::Part {
                height: part.height,
                round: part.round,
                part: decoded,
            })
        }
        message::Sum::Vote(vote) => {
            let proto = vote
                .vote
                .as_ref()
                .ok_or_else(|| Error::CorruptWal("missing vote".to_owned()))?;
            Vote::try_from_proto(proto)
                .map(Replay::Vote)
                .map_err(|err| Error::CorruptWal(err.to_string()))
        }
        _ => Ok(Replay::Ignored),
    }
}

fn timed(time: Time, sum: wal_message::Sum) -> eld_tendermint_proto::consensus::TimedWalMessage {
    eld_tendermint_proto::consensus::TimedWalMessage {
        time: Some(time.to_prost()),
        msg: Some(WalMessage { sum: Some(sum) }),
    }
}

fn end_height_of(msg: &eld_tendermint_proto::consensus::TimedWalMessage) -> Option<i64> {
    match msg.msg.as_ref().and_then(|msg| msg.sum.as_ref()) {
        Some(wal_message::Sum::EndHeight(end)) => Some(end.height),
        _ => None,
    }
}

fn encode(msg: &eld_tendermint_proto::consensus::TimedWalMessage) -> Result<Vec<u8>, Error> {
    let data = msg.encode_to_vec();
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    if length > MAX_MSG_SIZE_BYTES {
        return Err(Error::CorruptWal(format!(
            "msg is too big: {length} bytes, max: {MAX_MSG_SIZE_BYTES} bytes"
        )));
    }
    let crc = crc32c::crc32c(&data);
    let mut out = Vec::with_capacity(8 + data.len());
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&data);
    Ok(out)
}

/// `Ok(None)` is a clean EOF on the first byte of the CRC.
fn decode_one(
    reader: &mut impl Read,
) -> Result<Option<eld_tendermint_proto::consensus::TimedWalMessage>, Error> {
    let Some(crc) = read_u32(reader, true)? else {
        return Ok(None);
    };
    let Some(length) = read_u32(reader, false)? else {
        return Err(Error::CorruptWal("failed to read length".to_owned()));
    };
    if length > MAX_MSG_SIZE_BYTES {
        return Err(Error::CorruptWal(format!(
            "length {length} exceeded maximum possible value of {MAX_MSG_SIZE_BYTES} bytes"
        )));
    }
    let mut data = vec![0_u8; length as usize];
    reader
        .read_exact(&mut data)
        .map_err(|err| Error::CorruptWal(format!("failed to read data: {err}")))?;
    let actual = crc32c::crc32c(&data);
    if actual != crc {
        return Err(Error::CorruptWal(format!(
            "checksums do not match: read: {crc}, actual: {actual}"
        )));
    }
    eld_tendermint_proto::consensus::TimedWalMessage::decode(data.as_slice())
        .map(Some)
        .map_err(|err| Error::CorruptWal(format!("failed to decode data: {err}")))
}

/// `allow_eof` is true only for the CRC word, matching `WALDecoder.Decode`.
fn read_u32(reader: &mut impl Read, allow_eof: bool) -> Result<Option<u32>, Error> {
    let mut buf = [0_u8; 4];
    let mut filled = 0;
    while filled < 4 {
        match reader.read(&mut buf[filled..]) {
            Ok(0) if filled == 0 && allow_eof => return Ok(None),
            Ok(0) => {
                return Err(Error::CorruptWal(if allow_eof {
                    "failed to read checksum".to_owned()
                } else {
                    "failed to read length".to_owned()
                }));
            }
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(Error::CorruptWal(err.to_string())),
        }
    }
    Ok(Some(u32::from_be_bytes(buf)))
}
