//! Dial a remote privval signer.
//!
//! TCP uses the secret-connection handshake from `DialTCPFn`. A unix socket is
//! a plain stream, matching `DialUnixFn`. Frames are an unsigned varint length
//! plus a `tendermint.privval.Message`, at most 10 KiB.

use std::io::{Read, Write};
use std::net::TcpStream;

use eld_tendermint_crypto::{PrivKey, PubKey, pub_key_from_proto};
use eld_tendermint_p2p::make_secret_connection;
use eld_tendermint_proto::privval::{
    Message as PvMessage, PubKeyRequest, PubKeyResponse, RemoteSignerError, SignProposalRequest,
    SignVoteRequest, SignedProposalResponse, SignedVoteResponse, message::Sum,
};
use eld_tendermint_types::{Proposal, Time, Vote};
use prost::Message;

use crate::ensured::Ensured;
use crate::{Error, FilePVLastSignState, PrivValidator, STEP_NONE, STEP_PROPOSE, vote_step};

/// `maxRemoteSignerMsgSize`.
const MAX_MSG_SIZE: u64 = 10 * 1024;

trait Io: Read + Write + Send {}

impl<T: Read + Write + Send> Io for T {}

/// `SignerClient` for a node that dials `priv_validator_laddr`.
pub struct RemoteSigner {
    conn: Box<dyn Io>,
    pub_key: PubKey,
    last_sign_state: FilePVLastSignState,
}

impl RemoteSigner {
    /// Connect once, then `GetPubKey`. A refused dial is returned to the caller.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dial`] when the address cannot be connected, [`Error::Remote`]
    /// when the signer rejects the chain id or the key, or [`Error::UnexpectedResponse`]
    /// when the reply is not a public key.
    pub fn dial(addr: &str, chain_id: &str) -> Result<Self, Error> {
        let mut conn = connect(addr)?;
        let pub_key = request_pub_key(&mut conn, chain_id)?;
        Ok(Self {
            conn,
            pub_key,
            last_sign_state: FilePVLastSignState {
                height: 0,
                round: 0,
                step: STEP_NONE,
                signature: None,
                sign_bytes: None,
            },
        })
    }

    fn exchange(&mut self, request: PvMessage) -> Result<PvMessage, Error> {
        write_delimited(&mut *self.conn, &request)?;
        read_delimited(&mut *self.conn)
    }

    fn reject_different(
        &self,
        height: i64,
        round: i32,
        step: i8,
        signature: &[u8],
    ) -> Result<(), Error> {
        let state = &self.last_sign_state;
        let same = state.height == height && state.round == round && state.step == step;
        if same
            && state
                .signature
                .as_deref()
                .is_some_and(|stored| stored != signature)
        {
            return Err(Error::ConflictingData);
        }
        Ok(())
    }

    fn remember(
        &mut self,
        height: i64,
        round: i32,
        step: i8,
        signature: Vec<u8>,
        sign_bytes: Vec<u8>,
    ) {
        self.last_sign_state.height = height;
        self.last_sign_state.round = round;
        self.last_sign_state.step = step;
        self.last_sign_state.signature = Some(signature);
        self.last_sign_state.sign_bytes = Some(sign_bytes);
    }
}

impl PrivValidator for RemoteSigner {
    fn get_pub_key(&self) -> PubKey {
        self.pub_key
    }

    fn sign_vote(&mut self, chain_id: &str, vote: &mut Vote) -> Result<(), Error> {
        let step = vote_step(vote.vote_type)?;
        let response = self.exchange(sign_vote_message(chain_id, vote))?;
        let Sum::SignedVoteResponse(SignedVoteResponse {
            vote: signed,
            error,
        }) = response.sum.ok_or(Error::UnexpectedResponse)?
        else {
            return Err(Error::UnexpectedResponse);
        };
        if let Some(error) = error {
            return Err(remote_failure(error));
        }
        let signed = signed.ok_or(Error::UnexpectedResponse)?;
        self.reject_different(vote.height, vote.round, step, &signed.signature)?;
        if signed.timestamp.is_some() {
            vote.timestamp = Time::from_prost(signed.timestamp.as_ref());
        }
        vote.signature = signed.signature.clone();
        let sign_bytes = vote.sign_bytes(chain_id);
        self.remember(vote.height, vote.round, step, signed.signature, sign_bytes);
        Ok(())
    }

    fn sign_proposal(&mut self, chain_id: &str, proposal: &mut Proposal) -> Result<(), Error> {
        let response = self.exchange(sign_proposal_message(chain_id, proposal))?;
        let Sum::SignedProposalResponse(SignedProposalResponse {
            proposal: signed,
            error,
        }) = response.sum.ok_or(Error::UnexpectedResponse)?
        else {
            return Err(Error::UnexpectedResponse);
        };
        if let Some(error) = error {
            return Err(remote_failure(error));
        }
        let signed = signed.ok_or(Error::UnexpectedResponse)?;
        self.reject_different(
            proposal.height,
            proposal.round,
            STEP_PROPOSE,
            &signed.signature,
        )?;
        if signed.timestamp.is_some() {
            proposal.timestamp = Time::from_prost(signed.timestamp.as_ref());
        }
        proposal.signature = signed.signature.clone();
        let sign_bytes = proposal.sign_bytes(chain_id);
        self.remember(
            proposal.height,
            proposal.round,
            STEP_PROPOSE,
            signed.signature,
            sign_bytes,
        );
        Ok(())
    }

    fn last_sign_state(&self) -> &FilePVLastSignState {
        &self.last_sign_state
    }
}

/// `SignVoteRequest` wrapped as `privval.Message`.
#[must_use]
pub fn sign_vote_message(chain_id: &str, vote: &Vote) -> PvMessage {
    PvMessage {
        sum: Some(Sum::SignVoteRequest(SignVoteRequest {
            vote: Some(vote.to_proto()),
            chain_id: chain_id.to_owned(),
        })),
    }
}

fn sign_proposal_message(chain_id: &str, proposal: &Proposal) -> PvMessage {
    PvMessage {
        sum: Some(Sum::SignProposalRequest(SignProposalRequest {
            proposal: Some(proposal.to_proto()),
            chain_id: chain_id.to_owned(),
        })),
    }
}

fn request_pub_key(conn: &mut dyn Io, chain_id: &str) -> Result<PubKey, Error> {
    let request = PvMessage {
        sum: Some(Sum::PubKeyRequest(PubKeyRequest {
            chain_id: chain_id.to_owned(),
        })),
    };
    write_delimited(conn, &request)?;
    let response = read_delimited(conn)?;
    let Sum::PubKeyResponse(PubKeyResponse { pub_key, error }) =
        response.sum.ok_or(Error::UnexpectedResponse)?
    else {
        return Err(Error::UnexpectedResponse);
    };
    if let Some(error) = error {
        return Err(remote_failure(error));
    }
    let pub_key = pub_key.ok_or(Error::UnexpectedResponse)?;
    pub_key_from_proto(&pub_key).map_err(Error::Key)
}

fn remote_failure(error: RemoteSignerError) -> Error {
    if error.description == "conflicting data" {
        Error::ConflictingData
    } else {
        Error::Remote {
            description: error.description,
        }
    }
}

fn connect(addr: &str) -> Result<Box<dyn Io>, Error> {
    let (protocol, address) = protocol_and_address(addr);
    match protocol {
        "tcp" => {
            let stream = TcpStream::connect(address).map_err(|err| Error::Dial {
                address: addr.to_owned(),
                message: err.to_string(),
            })?;
            let _ = stream.set_nodelay(true);
            let key = PrivKey::generate();
            let secret = make_secret_connection(stream, &key).map_err(|err| Error::Dial {
                address: addr.to_owned(),
                message: err.to_string(),
            })?;
            Ok(Box::new(secret))
        }
        #[cfg(unix)]
        "unix" => {
            let stream =
                std::os::unix::net::UnixStream::connect(address).map_err(|err| Error::Dial {
                    address: addr.to_owned(),
                    message: err.to_string(),
                })?;
            Ok(Box::new(stream))
        }
        other => Err(Error::Dial {
            address: addr.to_owned(),
            message: format!("unknown protocol {other}"),
        }),
    }
}

/// `ProtocolAndAddress`. A missing scheme is `tcp`.
fn protocol_and_address(addr: &str) -> (&str, &str) {
    match addr.split_once("://") {
        Some((protocol, address)) => (protocol, address),
        None => ("tcp", addr),
    }
}

/// Unsigned-varint length, then the protobuf body. Not the ABCI zigzag prefix.
///
/// # Errors
///
/// Returns [`Error::Socket`] when the write fails or the message exceeds 10 KiB.
pub fn write_delimited<W: Write + ?Sized>(writer: &mut W, msg: &PvMessage) -> Result<(), Error> {
    let bytes = msg.encode_to_vec();
    let len = u64::try_from(bytes.len()).map_err(|_| Error::Socket("message length".to_owned()))?;
    if len > MAX_MSG_SIZE {
        return Err(Error::Socket(format!("message is {len} bytes")));
    }
    let mut len_buf = [0u8; 10];
    let n = put_uvarint(&mut len_buf, len);
    writer
        .write_all(&len_buf[..n])
        .map_err(|err| Error::Socket(err.to_string()))?;
    writer
        .write_all(&bytes)
        .map_err(|err| Error::Socket(err.to_string()))?;
    writer.flush().map_err(|err| Error::Socket(err.to_string()))
}

/// Inverse of [`write_delimited`].
///
/// # Errors
///
/// Returns [`Error::Socket`] when the frame is short, too long, or not a message.
pub fn read_delimited<R: Read + ?Sized>(reader: &mut R) -> Result<PvMessage, Error> {
    let len = read_uvarint(reader)?;
    if len > MAX_MSG_SIZE {
        return Err(Error::Socket(format!("message is {len} bytes")));
    }
    let mut bytes =
        vec![0u8; usize::try_from(len).map_err(|_| Error::Socket("message length".to_owned()))?];
    reader
        .read_exact(&mut bytes)
        .map_err(|err| Error::Socket(err.to_string()))?;
    PvMessage::decode(bytes.as_slice()).map_err(|err| Error::Socket(err.to_string()))
}

fn put_uvarint(buf: &mut [u8], mut value: u64) -> usize {
    let mut i = 0;
    while value >= 0x80 {
        buf[i] = u8::try_from(value & 0x7f).ensured("low 7 bits") | 0x80;
        value >>= 7;
        i += 1;
    }
    buf[i] = u8::try_from(value).ensured("last varint byte");
    i + 1
}

fn read_uvarint<R: Read + ?Sized>(reader: &mut R) -> Result<u64, Error> {
    let mut value = 0u64;
    let mut shift = 0;
    for _ in 0..10 {
        let mut buf = [0u8; 1];
        reader
            .read_exact(&mut buf)
            .map_err(|err| Error::Socket(err.to_string()))?;
        let byte = buf[0];
        if shift == 63 && byte > 1 {
            return Err(Error::Socket("varint overflow".to_owned()));
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(Error::Socket("varint overflow".to_owned()))
}
