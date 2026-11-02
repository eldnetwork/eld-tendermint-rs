//! Consensus WAL. CRC32C-framed `TimedWALMessage` records.
//!
//! Format, from `consensus/wal.go`: 4-byte Castagnoli CRC, 4-byte big-endian length,
//! then the protobuf bytes. The CRC covers only the protobuf. The head stays the
//! configured path (`data/cs.wal/wal`). At 10 MiB it is renamed to `wal.NNN`, matching
//! `autofile.Group.RotateFile`, and replaced by an empty file. Each append is
//! `fsync`ed before it returns, so the size check runs on that write.

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

use crate::error::{Error, WalCorrupt};

/// `maxMsgSize + 24` from `consensus/wal.go`.
const MAX_MSG_SIZE_BYTES: u32 = 1_048_576 + 24;

/// `defaultHeadSizeLimit` in `libs/autofile/group.go`.
const HEAD_SIZE_LIMIT: u64 = 10 * 1024 * 1024;

/// Append-only consensus WAL. The head is `path`; rotated segments are `path.NNN`.
pub struct Wal {
    path: PathBuf,
    file: File,
}

impl Wal {
    /// Opens `path`, creating parent directories. An empty head gets `EndHeight { 0 }`
    /// only when no rotated segment exists.
    ///
    /// # Errors
    ///
    /// Returns an I/O error, or a WAL error if the initial marker cannot be written.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(io_err)?;
            }
        }
        let file = open_append(&path)?;
        let empty = file.metadata().map_err(io_err)?.len() == 0;
        let mut wal = Self { path, file };
        if empty && segment_indexes(&wal.path)?.is_empty() {
            wal.write_message(&end_height_message(Time::now(), 0))?;
        }
        Ok(wal)
    }

    /// Encodes `msg`, appends it, and `fsync`s before returning.
    ///
    /// A head that has reached 10 MiB is rotated after the record is durable.
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
        self.file.write_all(&bytes).map_err(io_err)?;
        self.file.sync_all().map_err(io_err)?;
        if self.file.metadata().map_err(io_err)?.len() >= HEAD_SIZE_LIMIT {
            self.rotate()?;
        }
        Ok(())
    }

    /// Renames the head to `{path}.{index:03}` and replaces it with an empty file.
    ///
    /// The new head is written to a sibling temp file, `fsync`ed, and renamed into
    /// place. `EndHeight { 0 }` is not written into that head.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when the rename or the replacement head cannot be written.
    pub fn rotate(&mut self) -> Result<(), Error> {
        self.file.sync_all().map_err(io_err)?;
        let index = next_segment_index(&self.path)?;
        let segment = segment_path(&self.path, index);
        let parked = File::open("/dev/null").map_err(io_err)?;
        drop(std::mem::replace(&mut self.file, parked));
        if let Err(err) = std::fs::rename(&self.path, &segment) {
            self.file = open_append(&self.path)?;
            return Err(io_err(err));
        }
        if let Err(err) = replace_with_empty(&self.path) {
            self.file = open_append(&self.path)?;
            return Err(err);
        }
        self.file = open_append(&self.path)?;
        Ok(())
    }

    /// Every record in the head file, in order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptWal`] on a torn or mismatched record.
    pub fn read_messages(
        &self,
    ) -> Result<Vec<eld_tendermint_proto::consensus::TimedWalMessage>, Error> {
        read_messages_at(&self.path)
    }

    /// Records after the first `EndHeight` for `height`. `None` when that marker is absent.
    /// The marker itself is not included.
    ///
    /// The head is scanned first. Older `wal.NNN` segments are read, newest first, only
    /// when the head does not contain the marker. Records after the marker include the
    /// rest of that segment and every newer file, including the head.
    ///
    /// # Errors
    ///
    /// Returns [`Error::CorruptWal`] on a bad record.
    pub fn messages_after_end_height(
        &self,
        height: i64,
    ) -> Result<Option<Vec<eld_tendermint_proto::consensus::TimedWalMessage>>, Error> {
        let head = read_messages_at(&self.path)?;
        if let Some(pos) = end_height_index(&head, height) {
            return Ok(Some(head[pos + 1..].to_vec()));
        }
        let mut last_height_found = last_end_height(&head);
        if last_height_found > 0 && last_height_found < height {
            return Ok(None);
        }
        let mut indexes = segment_indexes(&self.path)?;
        indexes.sort_unstable_by(|left, right| right.cmp(left));
        let mut passed: Vec<Vec<eld_tendermint_proto::consensus::TimedWalMessage>> = Vec::new();
        for index in indexes {
            let messages = read_messages_at(&segment_path(&self.path, index))?;
            if let Some(pos) = end_height_index(&messages, height) {
                let mut after = messages[pos + 1..].to_vec();
                for newer in passed.iter().rev() {
                    after.extend(newer.iter().cloned());
                }
                after.extend(head);
                return Ok(Some(after));
            }
            if let Some(last) = messages.iter().rev().find_map(end_height_of) {
                last_height_found = last;
            }
            if last_height_found > 0 && last_height_found < height {
                return Ok(None);
            }
            passed.push(messages);
        }
        Ok(None)
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
        return Err(Error::CorruptWal(WalCorrupt::EmptyMessage));
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
        return Err(Error::CorruptWal(WalCorrupt::EmptyMsgInfo));
    };
    match sum {
        message::Sum::Proposal(proposal) => {
            let proto = proposal
                .proposal
                .as_ref()
                .ok_or(Error::CorruptWal(WalCorrupt::MissingProposal))?;
            Proposal::try_from_proto(proto)
                .map(Replay::Proposal)
                .map_err(|err| Error::CorruptWal(WalCorrupt::Decode(err.to_string())))
        }
        message::Sum::BlockPart(part) => {
            let decoded = Part::try_from_proto(part.part.as_ref())
                .map_err(|err| Error::CorruptWal(WalCorrupt::Decode(err.to_string())))?;
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
                .ok_or(Error::CorruptWal(WalCorrupt::MissingVote))?;
            Vote::try_from_proto(proto)
                .map(Replay::Vote)
                .map_err(|err| Error::CorruptWal(WalCorrupt::Decode(err.to_string())))
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

fn end_height_index(
    messages: &[eld_tendermint_proto::consensus::TimedWalMessage],
    height: i64,
) -> Option<usize> {
    messages
        .iter()
        .position(|msg| end_height_of(msg) == Some(height))
}

/// `-1` when the slice has no `EndHeight`, matching `SearchForEndHeight`.
fn last_end_height(messages: &[eld_tendermint_proto::consensus::TimedWalMessage]) -> i64 {
    messages.iter().rev().find_map(end_height_of).unwrap_or(-1)
}

fn io_err(err: std::io::Error) -> Error {
    Error::Io(err)
}

fn open_append(path: &Path) -> Result<File, Error> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io_err)
}

fn read_messages_at(
    path: &Path,
) -> Result<Vec<eld_tendermint_proto::consensus::TimedWalMessage>, Error> {
    let mut file = File::open(path).map_err(io_err)?;
    let mut out = Vec::new();
    loop {
        match decode_one(&mut file)? {
            None => return Ok(out),
            Some(msg) => out.push(msg),
        }
    }
}

/// Writes an empty file to `.{name}.tmp`, `fsync`s it, and renames it onto `path`.
fn replace_with_empty(path: &Path) -> Result<(), Error> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("wal");
    let tmp_path = parent.join(format!(".{file_name}.tmp"));
    {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(&tmp_path).map_err(io_err)?;
        file.sync_all().map_err(io_err)?;
    }
    std::fs::rename(&tmp_path, path).map_err(io_err)?;
    Ok(())
}

fn segment_path(head: &Path, index: u32) -> PathBuf {
    let mut name = head.as_os_str().to_os_string();
    name.push(format!(".{index:03}"));
    PathBuf::from(name)
}

fn next_segment_index(head: &Path) -> Result<u32, Error> {
    let indexes = segment_indexes(head)?;
    indexes.into_iter().max().map_or(Ok(0), |index| {
        index
            .checked_add(1)
            .ok_or_else(|| Error::Io(std::io::Error::other("WAL segment index overflow")))
    })
}

fn segment_indexes(head: &Path) -> Result<Vec<u32>, Error> {
    let Some(base) = head.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let parent = head
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(io_err(err)),
    };
    let mut indexes = Vec::new();
    for entry in entries {
        let name = entry.map_err(io_err)?.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(index) = segment_index(name, base) {
            indexes.push(index);
        }
    }
    Ok(indexes)
}

/// `wal.000` for a head named `wal`. The suffix is at least three digits, as in
/// `autofile` group scan.
fn segment_index(name: &str, base: &str) -> Option<u32> {
    let digits = name.strip_prefix(base)?.strip_prefix('.')?;
    if digits.len() < 3 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn encode(msg: &eld_tendermint_proto::consensus::TimedWalMessage) -> Result<Vec<u8>, Error> {
    let data = msg.encode_to_vec();
    let length = u32::try_from(data.len()).unwrap_or(u32::MAX);
    if length > MAX_MSG_SIZE_BYTES {
        return Err(Error::CorruptWal(WalCorrupt::MsgTooBig {
            length,
            max: MAX_MSG_SIZE_BYTES,
        }));
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
        return Err(Error::CorruptWal(WalCorrupt::MissingLength));
    };
    if length > MAX_MSG_SIZE_BYTES {
        return Err(Error::CorruptWal(WalCorrupt::LengthTooBig {
            length,
            max: MAX_MSG_SIZE_BYTES,
        }));
    }
    let mut data = vec![0_u8; length as usize];
    reader
        .read_exact(&mut data)
        .map_err(|err| Error::CorruptWal(WalCorrupt::ShortRead(err)))?;
    let actual = crc32c::crc32c(&data);
    if actual != crc {
        return Err(Error::CorruptWal(WalCorrupt::Checksum {
            read: crc,
            actual,
        }));
    }
    eld_tendermint_proto::consensus::TimedWalMessage::decode(data.as_slice())
        .map(Some)
        .map_err(|err| Error::CorruptWal(WalCorrupt::Proto(err)))
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
                    WalCorrupt::ShortChecksum
                } else {
                    WalCorrupt::MissingLength
                }));
            }
            Ok(n) => filled += n,
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => {}
            Err(err) => return Err(Error::CorruptWal(WalCorrupt::Io(err))),
        }
    }
    Ok(Some(u32::from_be_bytes(buf)))
}
