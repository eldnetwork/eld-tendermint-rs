//! `abci/types.WriteMessage` and `ReadMessage`.
//!
//! The length prefix is `encoding/binary.PutVarint`: for a non-negative length `n`,
//! the unsigned varint of `n << 1`. That is not prost's length-delimited encoding.

use std::io::{Read, Write};

use prost::Message;

use crate::Error;

/// `maxMsgSize` (100 MiB).
const MAX_MSG_SIZE: i64 = 104_857_600;

/// `WriteMessage`.
///
/// # Errors
///
/// Returns [`Error::MessageTooBig`] when the encoded length does not fit in `i64`,
/// or [`Error::Io`] when the write fails.
pub fn write_message<W: Write, M: Message>(writer: &mut W, msg: &M) -> Result<(), Error> {
    let bytes = msg.encode_to_vec();
    let len = i64::try_from(bytes.len()).map_err(|_| Error::MessageTooBig { len: i64::MAX })?;
    let mut buf = Vec::with_capacity(10 + bytes.len());
    put_varint(&mut buf, len);
    buf.extend_from_slice(&bytes);
    writer.write_all(&buf).map_err(Error::Io)
}

/// `ReadMessage`.
///
/// # Errors
///
/// Returns [`Error::InvalidVarint`], [`Error::MessageTooBig`], [`Error::Io`], or
/// [`Error::Proto`].
pub fn read_message<R: Read, M: Message + Default>(reader: &mut R) -> Result<M, Error> {
    let len = read_varint(reader)?;
    if !(0..=MAX_MSG_SIZE).contains(&len) {
        return Err(Error::MessageTooBig { len });
    }
    let mut buf = vec![0u8; usize::try_from(len).expect("length fits in usize")];
    reader.read_exact(&mut buf).map_err(Error::Io)?;
    M::decode(buf.as_slice()).map_err(|err| Error::Proto(err.to_string()))
}

/// `encoding/binary.PutVarint`.
fn put_varint(buf: &mut Vec<u8>, value: i64) {
    let mut unsigned = u64::from_ne_bytes(value.to_ne_bytes()) << 1;
    if value < 0 {
        unsigned = !unsigned;
    }
    put_uvarint(buf, unsigned);
}

fn put_uvarint(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push((value as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

/// `encoding/binary.ReadVarint`.
fn read_varint<R: Read>(reader: &mut R) -> Result<i64, Error> {
    let unsigned = read_uvarint(reader)?;
    let mut value = (unsigned >> 1) as i64;
    if unsigned & 1 != 0 {
        value = !value;
    }
    Ok(value)
}

/// `encoding/binary.ReadUvarint`.
fn read_uvarint<R: Read>(reader: &mut R) -> Result<u64, Error> {
    let mut value = 0u64;
    let mut shift = 0;
    for i in 0..10 {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).map_err(Error::Io)?;
        let byte = byte[0];
        if byte < 0x80 {
            if i == 9 && byte > 1 {
                return Err(Error::InvalidVarint);
            }
            return Ok(value | u64::from(byte) << shift);
        }
        value |= u64::from(byte & 0x7f) << shift;
        shift += 7;
    }
    Err(Error::InvalidVarint)
}
