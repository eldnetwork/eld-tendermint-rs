//! `conn.SecretConnection`.
//!
//! Ephemeral keys are fresh X25519 (`nacl/box.GenerateKey`), not the node Ed25519 key.
//! The node key only signs the Merlin challenge. Frames are 1028 zero-padded plaintext
//! bytes: a little-endian length, then the chunk. ChaCha20-Poly1305 adds a 16-byte tag.

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

use chacha20poly1305::aead::generic_array::GenericArray;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use eld_tendermint_crypto::{PrivKey, PubKey, pub_key_from_proto, pub_key_to_proto};
use eld_tendermint_proto::crypto::PublicKey as ProtoPublicKey;
use eld_tendermint_proto::p2p::AuthSigMessage;
use hkdf::Hkdf;
use merlin::Transcript;
use prost::Message;
use rand::rngs::OsRng;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, SharedSecret};

use crate::Error;

const DATA_LEN_SIZE: usize = 4;
const DATA_MAX_SIZE: usize = 1024;
const TOTAL_FRAME_SIZE: usize = DATA_MAX_SIZE + DATA_LEN_SIZE;
const AEAD_TAG_SIZE: usize = 16;
const SEALED_FRAME_SIZE: usize = TOTAL_FRAME_SIZE + AEAD_TAG_SIZE;
const AEAD_KEY_SIZE: usize = 32;
const AEAD_NONCE_SIZE: usize = 12;
const DELIMITED_MAX: usize = 1024 * 1024;

const LABEL_EPH_LOWER: &[u8] = b"EPHEMERAL_LOWER_PUBLIC_KEY";
const LABEL_EPH_UPPER: &[u8] = b"EPHEMERAL_UPPER_PUBLIC_KEY";
const LABEL_DH_SECRET: &[u8] = b"DH_SECRET";
const LABEL_MAC: &[u8] = b"SECRET_CONNECTION_MAC";
const TRANSCRIPT: &[u8] = b"TENDERMINT_SECRET_CONNECTION_TRANSCRIPT_HASH";
const HKDF_INFO: &[u8] = b"TENDERMINT_SECRET_CONNECTION_KEY_AND_CHALLENGE_GEN";

/// `google.protobuf.BytesValue`. prost-types 0.13 does not ship this message.
#[derive(Clone, PartialEq, Message)]
struct BytesValue {
    #[prost(bytes = "vec", tag = "1")]
    value: Vec<u8>,
}

/// `deriveSecrets`.
///
/// HKDF-SHA256 with an empty salt and info `TENDERMINT_SECRET_CONNECTION_KEY_AND_CHALLENGE_GEN`.
/// Reads 96 bytes. When `loc_is_least` is true, recv is `[0..32)` and send is `[32..64)`.
/// Otherwise those halves swap. The last 32 bytes are unused.
///
/// HKDF expand of a fixed 96-byte output does not fail. Go panics if the read fails.
#[must_use]
pub fn derive_secrets(dh_secret: &[u8; 32], loc_is_least: bool) -> ([u8; 32], [u8; 32]) {
    let hkdf = Hkdf::<Sha256>::new(None, dh_secret);
    let mut okm = [0u8; 2 * AEAD_KEY_SIZE + 32];
    hkdf.expand(HKDF_INFO, &mut okm)
        .expect("HKDF-SHA256 expands 96 bytes");
    let mut recv = [0u8; AEAD_KEY_SIZE];
    let mut send = [0u8; AEAD_KEY_SIZE];
    if loc_is_least {
        recv.copy_from_slice(&okm[..AEAD_KEY_SIZE]);
        send.copy_from_slice(&okm[AEAD_KEY_SIZE..2 * AEAD_KEY_SIZE]);
    } else {
        send.copy_from_slice(&okm[..AEAD_KEY_SIZE]);
        recv.copy_from_slice(&okm[AEAD_KEY_SIZE..2 * AEAD_KEY_SIZE]);
    }
    (recv, send)
}

/// `conn.SecretConnection` after a successful handshake.
pub struct SecretConnection<S> {
    conn: S,
    recv_aead: ChaCha20Poly1305,
    send_aead: ChaCha20Poly1305,
    recv_nonce: [u8; AEAD_NONCE_SIZE],
    send_nonce: [u8; AEAD_NONCE_SIZE],
    recv_buffer: Vec<u8>,
    remote_pub_key: PubKey,
}

impl<S> SecretConnection<S> {
    /// Authenticated remote Ed25519 public key.
    #[must_use]
    pub fn remote_pub_key(&self) -> &PubKey {
        &self.remote_pub_key
    }
}

impl<S: SplitIo> SecretConnection<S> {
    /// Split the authenticated connection into a reader, a writer, and a shutdown handle.
    ///
    /// `MConnection` reads and writes on two threads. One socket cannot do both: a blocked
    /// `read` holds the only handle, and the send thread never writes. `UnixStream::try_clone`
    /// dups the fd so one half reads, one writes, and `shutdown(Both)` unblocks a stuck read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the socket cannot be duplicated.
    pub fn split(self) -> Result<SplitParts<S>, Error> {
        let (read_conn, write_conn, shutdown) = self.conn.split_io().map_err(Error::Io)?;
        Ok((
            SecretReader {
                conn: read_conn,
                recv_aead: self.recv_aead,
                recv_nonce: self.recv_nonce,
                recv_buffer: self.recv_buffer,
                remote_pub_key: self.remote_pub_key,
            },
            SecretWriter {
                conn: write_conn,
                send_aead: self.send_aead,
                send_nonce: self.send_nonce,
            },
            shutdown,
        ))
    }
}

/// Read half of a [`SecretConnection`]. Decrypts ChaCha20-Poly1305 frames.
pub struct SecretReader<R> {
    conn: R,
    recv_aead: ChaCha20Poly1305,
    recv_nonce: [u8; AEAD_NONCE_SIZE],
    recv_buffer: Vec<u8>,
    remote_pub_key: PubKey,
}

impl<R> SecretReader<R> {
    /// Authenticated remote Ed25519 public key.
    #[must_use]
    pub fn remote_pub_key(&self) -> &PubKey {
        &self.remote_pub_key
    }
}

impl<R: Read> Read for SecretReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.recv_buffer.is_empty() {
            self.recv_buffer = open_frame(&mut self.conn, &self.recv_aead, &mut self.recv_nonce)
                .map_err(Error::into_io)?;
        }
        let n = buf.len().min(self.recv_buffer.len());
        buf[..n].copy_from_slice(&self.recv_buffer[..n]);
        self.recv_buffer.drain(..n);
        Ok(n)
    }
}

/// Write half of a [`SecretConnection`]. Seals ChaCha20-Poly1305 frames.
pub struct SecretWriter<W> {
    conn: W,
    send_aead: ChaCha20Poly1305,
    send_nonce: [u8; AEAD_NONCE_SIZE],
}

impl<W: Write> Write for SecretWriter<W> {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        while !data.is_empty() {
            let chunk_len = data.len().min(DATA_MAX_SIZE);
            seal_frame(
                &mut self.conn,
                &self.send_aead,
                &mut self.send_nonce,
                &data[..chunk_len],
            )
            .map_err(Error::into_io)?;
            data = &data[chunk_len..];
            written += chunk_len;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.conn.flush()
    }
}

/// Duplicates a connected socket so one half can block in `read` while the other writes.
pub trait SplitIo: Sized {
    /// Socket used only for reading.
    type ReadHalf: Read + Send + 'static;
    /// Socket used only for writing.
    type WriteHalf: Write + Send + 'static;
    /// Extra fd used to `shutdown` the connection from a third thread.
    type Shutdown: IoShutdown + 'static;

    /// # Errors
    ///
    /// Returns an error when the file descriptor cannot be duplicated.
    fn split_io(self) -> io::Result<(Self::ReadHalf, Self::WriteHalf, Self::Shutdown)>;
}

/// Reader, writer, and shutdown handle from [`SecretConnection::split`].
type SplitParts<S> = (
    SecretReader<<S as SplitIo>::ReadHalf>,
    SecretWriter<<S as SplitIo>::WriteHalf>,
    <S as SplitIo>::Shutdown,
);

/// Unblocks a peer thread blocked in `read` or `write`.
pub trait IoShutdown: Send + Sync {
    /// # Errors
    ///
    /// Returns an error when the socket is already closed.
    fn shutdown_io(&self) -> io::Result<()>;
}

impl SplitIo for UnixStream {
    type ReadHalf = UnixStream;
    type WriteHalf = UnixStream;
    type Shutdown = UnixStream;

    fn split_io(self) -> io::Result<(Self::ReadHalf, Self::WriteHalf, Self::Shutdown)> {
        let write = self.try_clone()?;
        let shutdown = self.try_clone()?;
        Ok((self, write, shutdown))
    }
}

impl IoShutdown for UnixStream {
    fn shutdown_io(&self) -> io::Result<()> {
        self.shutdown(Shutdown::Both)
    }
}

fn open_frame<R: Read>(
    conn: &mut R,
    recv_aead: &ChaCha20Poly1305,
    recv_nonce: &mut [u8; AEAD_NONCE_SIZE],
) -> Result<Vec<u8>, Error> {
    let mut sealed = [0u8; SEALED_FRAME_SIZE];
    conn.read_exact(&mut sealed).map_err(Error::Io)?;
    let plain = recv_aead
        .decrypt(Nonce::from_slice(recv_nonce), sealed.as_ref())
        .map_err(|_| Error::Decrypt)?;
    incr_nonce(recv_nonce)?;
    if plain.len() != TOTAL_FRAME_SIZE {
        return Err(Error::Decrypt);
    }
    let chunk_len = u32::from_le_bytes(plain[..DATA_LEN_SIZE].try_into().expect("4 bytes"));
    if chunk_len > u32::try_from(DATA_MAX_SIZE).expect("1024 fits in u32") {
        return Err(Error::ChunkTooBig);
    }
    let end = DATA_LEN_SIZE + chunk_len as usize;
    Ok(plain[DATA_LEN_SIZE..end].to_vec())
}

fn seal_frame<W: Write>(
    conn: &mut W,
    send_aead: &ChaCha20Poly1305,
    send_nonce: &mut [u8; AEAD_NONCE_SIZE],
    chunk: &[u8],
) -> Result<(), Error> {
    let mut frame = [0u8; TOTAL_FRAME_SIZE];
    let chunk_len = u32::try_from(chunk.len()).expect("chunk fits in u32");
    frame[..DATA_LEN_SIZE].copy_from_slice(&chunk_len.to_le_bytes());
    frame[DATA_LEN_SIZE..DATA_LEN_SIZE + chunk.len()].copy_from_slice(chunk);
    let sealed = send_aead
        .encrypt(Nonce::from_slice(send_nonce), frame.as_slice())
        .expect("ChaCha20-Poly1305 seals a 12-byte nonce");
    incr_nonce(send_nonce)?;
    conn.write_all(&sealed).map_err(Error::Io)
}

impl<S: Read + Write> Read for SecretConnection<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.recv_buffer.is_empty() {
            self.recv_buffer = open_frame(&mut self.conn, &self.recv_aead, &mut self.recv_nonce)
                .map_err(Error::into_io)?;
        }
        let n = buf.len().min(self.recv_buffer.len());
        buf[..n].copy_from_slice(&self.recv_buffer[..n]);
        self.recv_buffer.drain(..n);
        Ok(n)
    }
}

impl<S: Read + Write> Write for SecretConnection<S> {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let mut written = 0;
        while !data.is_empty() {
            let chunk_len = data.len().min(DATA_MAX_SIZE);
            seal_frame(
                &mut self.conn,
                &self.send_aead,
                &mut self.send_nonce,
                &data[..chunk_len],
            )
            .map_err(Error::into_io)?;
            data = &data[chunk_len..];
            written += chunk_len;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.conn.flush()
    }
}

/// `MakeSecretConnection`.
///
/// Writes the local ephemeral key, then reads the remote one. Both peers must run
/// together: each read waits for the other write. A socket buffer holds one message.
///
/// # Errors
///
/// Returns an I/O, protobuf, low-order, or challenge-verification error.
pub fn make_secret_connection<S: Read + Write>(
    mut conn: S,
    loc_priv_key: &PrivKey,
) -> Result<SecretConnection<S>, Error> {
    let loc_pub = loc_priv_key.public_key().map_err(Error::Crypto)?;

    let loc_eph_priv = EphemeralSecret::random_from_rng(OsRng);
    let loc_eph_bytes = PublicKey::from(&loc_eph_priv).to_bytes();

    write_msg(
        &mut conn,
        &BytesValue {
            value: loc_eph_bytes.to_vec(),
        },
    )?;
    let rem_msg: BytesValue = read_msg(&mut conn, DELIMITED_MAX)?;
    let rem_eph_bytes = copy32(&rem_msg.value);

    let (lo, hi) = sort32(&loc_eph_bytes, &rem_eph_bytes);
    let loc_is_least = loc_eph_bytes == lo;

    let shared = loc_eph_priv.diffie_hellman(&PublicKey::from(rem_eph_bytes));
    let dh_secret = shared_secret_bytes(&shared)?;

    let mut transcript = Transcript::new(TRANSCRIPT);
    transcript.append_message(LABEL_EPH_LOWER, &lo);
    transcript.append_message(LABEL_EPH_UPPER, &hi);
    transcript.append_message(LABEL_DH_SECRET, &dh_secret);
    let mut challenge = [0u8; 32];
    transcript.challenge_bytes(LABEL_MAC, &mut challenge);

    let (recv_secret, send_secret) = derive_secrets(&dh_secret, loc_is_least);
    let mut sc = SecretConnection {
        conn,
        recv_aead: ChaCha20Poly1305::new(GenericArray::from_slice(&recv_secret)),
        send_aead: ChaCha20Poly1305::new(GenericArray::from_slice(&send_secret)),
        recv_nonce: [0u8; AEAD_NONCE_SIZE],
        send_nonce: [0u8; AEAD_NONCE_SIZE],
        recv_buffer: Vec::new(),
        remote_pub_key: loc_pub,
    };

    let signature = loc_priv_key.sign(&challenge).map_err(Error::Crypto)?;
    write_msg(
        &mut sc,
        &AuthSigMessage {
            pub_key: Some(pub_key_to_proto(&loc_pub)),
            sig: signature.to_vec(),
        },
    )?;
    let rem_auth: AuthSigMessage = read_msg(&mut sc, DELIMITED_MAX)?;
    let proto = rem_auth.pub_key.unwrap_or(ProtoPublicKey { sum: None });
    let rem_pub = pub_key_from_proto(&proto).map_err(Error::Crypto)?;
    rem_pub
        .verify(&challenge, &rem_auth.sig)
        .map_err(|_| Error::ChallengeVerification)?;
    sc.remote_pub_key = rem_pub;
    Ok(sc)
}

fn shared_secret_bytes(shared: &SharedSecret) -> Result<[u8; 32], Error> {
    if !shared.was_contributory() {
        return Err(Error::LowOrderRemotePubKey);
    }
    Ok(shared.to_bytes())
}

fn sort32(loc: &[u8; 32], rem: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    if loc < rem {
        (*loc, *rem)
    } else {
        (*rem, *loc)
    }
}

fn copy32(src: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let n = src.len().min(out.len());
    out[..n].copy_from_slice(&src[..n]);
    out
}

fn incr_nonce(nonce: &mut [u8; AEAD_NONCE_SIZE]) -> Result<(), Error> {
    let counter = u64::from_le_bytes(nonce[4..].try_into().expect("8 nonce bytes"));
    if counter == u64::MAX {
        return Err(Error::NonceOverflow);
    }
    nonce[4..].copy_from_slice(&(counter + 1).to_le_bytes());
    Ok(())
}

pub(crate) fn write_msg<W: Write, M: Message>(writer: &mut W, msg: &M) -> Result<(), Error> {
    writer
        .write_all(&msg.encode_length_delimited_to_vec())
        .map_err(Error::Io)
}

pub(crate) fn read_msg<R: Read, M: Message + Default>(
    reader: &mut R,
    max_size: usize,
) -> Result<M, Error> {
    let len = read_uvarint(reader)?;
    if len > u64::try_from(max_size).expect("max size fits in u64") {
        return Err(Error::MessageTooBig { len });
    }
    let mut buf = vec![0u8; usize::try_from(len).expect("len checked against max size")];
    reader.read_exact(&mut buf).map_err(Error::Io)?;
    M::decode(buf.as_slice()).map_err(|err| Error::Proto(err.to_string()))
}

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
