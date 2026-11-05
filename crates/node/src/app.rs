//! ABCI sockets to the same app address: query, snapshot, mempool, consensus, and content.

use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_abci::SocketClient;
use eld_tendermint_consensus::SummaryQuery;
use eld_tendermint_crypto::sum;
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, RequestInfo,
    RequestInitChain, RequestQuery, ResponseBeginBlock, ResponseCheckTx, ResponseCommit,
    ResponseDeliverTx, ResponseEndBlock, ResponseInfo, ResponseInitChain, ResponseQuery,
};
use eld_tendermint_proto::content::message::Sum as ContentSum;
use eld_tendermint_state::{Error as StateError, TM_CORE_SEMVER};
use eld_tendermint_types::BLOCK_PROTOCOL;

use crate::content_wait::{ContentReply, ContentWaitKey, ContentWaiter};
use crate::wait::{DeliveredTx, TxWaiter};

struct Shared {
    query: Mutex<SocketClient>,
    mempool: Mutex<SocketClient>,
    consensus: Mutex<SocketClient>,
    content: Mutex<SocketClient>,
    /// Kept open. Go dials this client even when state sync is off.
    #[allow(dead_code)]
    snapshot: Mutex<SocketClient>,
    waiter: Arc<TxWaiter>,
    content_waiter: Arc<ContentWaiter>,
    /// Height from the latest `BeginBlock` header. `DeliverTx` reports this height.
    height: Mutex<i64>,
}

/// Query, mempool, consensus, and content sockets. Consensus and the mempool each hold a clone.
#[derive(Clone)]
pub struct AbciApp {
    shared: Arc<Shared>,
}

impl AbciApp {
    pub(crate) fn new(
        query: SocketClient,
        snapshot: SocketClient,
        mempool: SocketClient,
        consensus: SocketClient,
        content: SocketClient,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                query: Mutex::new(query),
                snapshot: Mutex::new(snapshot),
                mempool: Mutex::new(mempool),
                consensus: Mutex::new(consensus),
                content: Mutex::new(content),
                waiter: Arc::new(TxWaiter::new()),
                content_waiter: Arc::new(ContentWaiter::new()),
                height: Mutex::new(0),
            }),
        }
    }

    pub(crate) fn waiter(&self) -> Arc<TxWaiter> {
        Arc::clone(&self.shared.waiter)
    }

    pub(crate) fn content_waiter(&self) -> Arc<ContentWaiter> {
        Arc::clone(&self.shared.content_waiter)
    }

    pub(crate) fn init_chain(
        &self,
        request: RequestInitChain,
    ) -> Result<ResponseInitChain, StateError> {
        lock(&self.shared.consensus)
            .init_chain(request)
            .map_err(abci_err)
    }

    /// `QuerySync`. Height `0` is passed through; the app treats it as latest.
    ///
    /// # Errors
    ///
    /// Returns the socket error. The RPC layer turns that into an internal error.
    pub(crate) fn query(
        &self,
        request: RequestQuery,
    ) -> Result<ResponseQuery, eld_tendermint_abci::Error> {
        lock(&self.shared.query).query(request)
    }

    /// `InfoSync` on the query socket. Same connection Go uses for `ABCIInfo`.
    ///
    /// # Errors
    ///
    /// Returns the socket error. The RPC layer turns that into an internal error.
    pub(crate) fn info(&self) -> Result<ResponseInfo, eld_tendermint_abci::Error> {
        lock(&self.shared.query).info(info_request())
    }
}

impl SummaryQuery for AbciApp {
    fn query(&self, request: RequestQuery) -> Result<ResponseQuery, String> {
        AbciApp::query(self, request).map_err(|err| err.to_string())
    }
}

/// `proxy.RequestInfo`: core `0.34.24`, block protocol 11, p2p protocol 8.
pub(crate) fn info_request() -> RequestInfo {
    RequestInfo {
        version: TM_CORE_SEMVER.to_owned(),
        block_version: BLOCK_PROTOCOL,
        p2p_version: 8,
    }
}

impl eld_tendermint_mempool::App for AbciApp {
    fn check_tx(&mut self, request: RequestCheckTx) -> ResponseCheckTx {
        match lock(&self.shared.mempool).check_tx(request) {
            Ok(response) => response,
            Err(err) => ResponseCheckTx {
                code: 1,
                log: err.to_string(),
                ..ResponseCheckTx::default()
            },
        }
    }
}

impl eld_tendermint_content::App for AbciApp {
    fn handle_content_push(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::ContentPush,
    ) -> Result<eld_tendermint_proto::content::ContentPushAck, eld_tendermint_content::Error> {
        match forward_content(self, peer_id, ContentSum::ContentPush(msg))? {
            Some(ContentSum::ContentPushAck(ack)) => Ok(ack),
            _ => Err(content_rejected("content reply was not a receipt")),
        }
    }

    fn handle_content_request(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::ContentRequest,
    ) -> Result<Option<eld_tendermint_proto::content::ContentResponse>, eld_tendermint_content::Error>
    {
        match forward_content(self, peer_id, ContentSum::ContentRequest(msg))? {
            None => Ok(None),
            Some(ContentSum::ContentResponse(response)) => Ok(Some(response)),
            _ => Err(content_rejected("content reply was not a blob")),
        }
    }

    fn handle_capacity_challenge(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::CapacityChallenge,
    ) -> Result<
        eld_tendermint_proto::content::CapacityChallengeResponse,
        eld_tendermint_content::Error,
    > {
        match forward_content(self, peer_id, ContentSum::CapacityChallenge(msg))? {
            Some(ContentSum::CapacityChallengeResponse(response)) => Ok(response),
            _ => Err(content_rejected("content reply was not a proof")),
        }
    }

    fn handle_content_push_ack(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::ContentPushAck,
    ) -> Result<(), eld_tendermint_content::Error> {
        let request_id = msg.request_id;
        let provider = msg.provider.clone();
        forward_content(self, peer_id, ContentSum::ContentPushAck(msg))?;
        self.shared.content_waiter.notify(
            ContentWaitKey::Request(request_id),
            ContentReply::Ack { provider },
        );
        Ok(())
    }

    fn handle_content_response(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::ContentResponse,
    ) -> Result<(), eld_tendermint_content::Error> {
        let request_id = msg.request_id;
        let bytes = msg.blob.clone();
        forward_content(self, peer_id, ContentSum::ContentResponse(msg))?;
        self.shared.content_waiter.notify(
            ContentWaitKey::Request(request_id),
            ContentReply::Blob { bytes },
        );
        Ok(())
    }

    fn handle_capacity_challenge_response(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::CapacityChallengeResponse,
    ) -> Result<(), eld_tendermint_content::Error> {
        let challenge_id = msg.challenge_id.clone();
        forward_content(self, peer_id, ContentSum::CapacityChallengeResponse(msg))?;
        self.shared.content_waiter.notify(
            ContentWaitKey::Challenge(challenge_id.clone()),
            ContentReply::Proof { challenge_id },
        );
        Ok(())
    }

    fn handle_announce(
        &self,
        peer_id: &str,
        msg: eld_tendermint_proto::content::Announce,
    ) -> Result<(), eld_tendermint_content::Error> {
        forward_content(self, peer_id, ContentSum::Announce(msg)).map(|_| ())
    }

    fn capacity_proof_summary(
        &self,
        epoch: i64,
    ) -> Result<Option<Vec<u8>>, eld_tendermint_content::Error> {
        match forward_content(
            self,
            "",
            ContentSum::CapacityProofSummaryQuery(
                eld_tendermint_proto::content::CapacityProofSummaryQuery { epoch },
            ),
        )? {
            Some(ContentSum::CapacityProofSummary(body)) if !body.summary.is_empty() => {
                Ok(Some(body.summary))
            }
            Some(ContentSum::CapacityProofSummary(_)) | None => Ok(None),
            _ => Err(content_rejected("content reply was not a summary")),
        }
    }
}

fn forward_content(
    app: &AbciApp,
    peer_id: &str,
    sum: ContentSum,
) -> Result<Option<ContentSum>, eld_tendermint_content::Error> {
    let response = lock(&app.shared.content)
        .content_call(
            peer_id,
            eld_tendermint_proto::content::Message { sum: Some(sum) },
        )
        .map_err(|err| content_rejected(err.to_string()))?;
    Ok(response.message.and_then(|message| message.sum))
}

fn content_rejected(reason: impl Into<String>) -> eld_tendermint_content::Error {
    eld_tendermint_content::Error::Rejected {
        reason: reason.into(),
    }
}

impl eld_tendermint_state::App for AbciApp {
    fn begin_block(
        &mut self,
        request: RequestBeginBlock,
    ) -> Result<ResponseBeginBlock, StateError> {
        if let Some(header) = &request.header {
            *lock_height(&self.shared.height) = header.height;
        }
        lock(&self.shared.consensus)
            .begin_block(request)
            .map_err(abci_err)
    }

    fn deliver_tx(&mut self, request: RequestDeliverTx) -> Result<ResponseDeliverTx, StateError> {
        let hash = sum(request.tx.as_ref());
        let response = lock(&self.shared.consensus)
            .deliver_tx(request)
            .map_err(abci_err)?;
        let height = *lock_height(&self.shared.height);
        self.shared.waiter.notify(
            hash,
            DeliveredTx {
                height,
                response: response.clone(),
            },
        );
        Ok(response)
    }

    fn end_block(&mut self, request: RequestEndBlock) -> Result<ResponseEndBlock, StateError> {
        lock(&self.shared.consensus)
            .end_block(request)
            .map_err(abci_err)
    }

    fn commit(&mut self) -> Result<ResponseCommit, StateError> {
        lock(&self.shared.consensus).commit().map_err(abci_err)
    }
}

fn abci_err(err: eld_tendermint_abci::Error) -> StateError {
    StateError::Abci(err.to_string())
}

fn lock(client: &Mutex<SocketClient>) -> MutexGuard<'_, SocketClient> {
    client.lock().unwrap_or_else(|err| err.into_inner())
}

fn lock_height(height: &Mutex<i64>) -> MutexGuard<'_, i64> {
    height.lock().unwrap_or_else(|err| err.into_inner())
}
