//! `abci/client.socketClient` sync calls.
//!
//! Each call writes one `Request`, then `RequestFlush`, and reads those two responses.
//! The server keeps responses in a buffer until it sees flush.

use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};

use eld_tendermint_proto::abci::{
    Request, RequestBeginBlock, RequestCheckTx, RequestCommit, RequestDeliverTx, RequestEcho,
    RequestEndBlock, RequestFlush, RequestInfo, RequestInitChain, RequestQuery, Response,
    ResponseBeginBlock, ResponseCheckTx, ResponseCommit, ResponseDeliverTx, ResponseEcho,
    ResponseEndBlock, ResponseInfo, ResponseInitChain, ResponseQuery, request, response,
};

use crate::error::Error;
use crate::protoio::{read_message, write_message};

/// Blocking ABCI 0.17 socket client.
pub struct SocketClient {
    stream: TcpStream,
}

impl SocketClient {
    /// Dials `addr` once.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the connect fails. There is no retry.
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, Error> {
        let stream = TcpStream::connect(addr).map_err(Error::Io)?;
        Ok(Self { stream })
    }

    /// `EchoSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn echo(&mut self, message: impl Into<String>) -> Result<ResponseEcho, Error> {
        self.call(
            request::Value::Echo(RequestEcho {
                message: message.into(),
            }),
            |value| match value {
                response::Value::Echo(echo) => Ok(echo),
                _ => Err(Error::MismatchedResponse),
            },
        )
    }

    /// `InfoSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn info(&mut self, req: RequestInfo) -> Result<ResponseInfo, Error> {
        self.call(request::Value::Info(req), |value| match value {
            response::Value::Info(info) => Ok(info),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `CheckTxSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn check_tx(&mut self, req: RequestCheckTx) -> Result<ResponseCheckTx, Error> {
        self.call(request::Value::CheckTx(req), |value| match value {
            response::Value::CheckTx(check_tx) => Ok(check_tx),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `DeliverTxSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn deliver_tx(&mut self, req: RequestDeliverTx) -> Result<ResponseDeliverTx, Error> {
        self.call(request::Value::DeliverTx(req), |value| match value {
            response::Value::DeliverTx(deliver_tx) => Ok(deliver_tx),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `CommitSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn commit(&mut self) -> Result<ResponseCommit, Error> {
        self.call(
            request::Value::Commit(RequestCommit {}),
            |value| match value {
                response::Value::Commit(commit) => Ok(commit),
                _ => Err(Error::MismatchedResponse),
            },
        )
    }

    /// `QuerySync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn query(&mut self, req: RequestQuery) -> Result<ResponseQuery, Error> {
        self.call(request::Value::Query(req), |value| match value {
            response::Value::Query(query) => Ok(query),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `BeginBlockSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn begin_block(&mut self, req: RequestBeginBlock) -> Result<ResponseBeginBlock, Error> {
        self.call(request::Value::BeginBlock(req), |value| match value {
            response::Value::BeginBlock(begin_block) => Ok(begin_block),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `EndBlockSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn end_block(&mut self, req: RequestEndBlock) -> Result<ResponseEndBlock, Error> {
        self.call(request::Value::EndBlock(req), |value| match value {
            response::Value::EndBlock(end_block) => Ok(end_block),
            _ => Err(Error::MismatchedResponse),
        })
    }

    /// `InitChainSync`.
    ///
    /// # Errors
    ///
    /// Returns an I/O, exception, or [`Error::MismatchedResponse`] error.
    pub fn init_chain(&mut self, req: RequestInitChain) -> Result<ResponseInitChain, Error> {
        self.call(request::Value::InitChain(req), |value| match value {
            response::Value::InitChain(init_chain) => Ok(init_chain),
            _ => Err(Error::MismatchedResponse),
        })
    }

    fn call<T>(
        &mut self,
        request_value: request::Value,
        take: impl FnOnce(response::Value) -> Result<T, Error>,
    ) -> Result<T, Error> {
        write_message(
            &mut self.stream,
            &Request {
                value: Some(request_value),
            },
        )?;
        write_message(
            &mut self.stream,
            &Request {
                value: Some(request::Value::Flush(RequestFlush {})),
            },
        )?;
        let response = read_response(&mut self.stream)?;
        let flush = read_response(&mut self.stream)?;
        match flush {
            response::Value::Flush(_) => take(response),
            _ => Err(Error::MismatchedResponse),
        }
    }
}

/// Reads one response. `ResponseException` is returned as [`Error::Exception`] and is not
/// followed by another read. Go stops the client on that arm.
fn read_response<R: Read>(reader: &mut R) -> Result<response::Value, Error> {
    let response: Response = read_message(reader)?;
    match response.value {
        Some(response::Value::Exception(exception)) => Err(Error::Exception(exception.error)),
        Some(value) => Ok(value),
        None => Err(Error::MismatchedResponse),
    }
}
