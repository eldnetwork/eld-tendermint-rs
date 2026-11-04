//! Go's four ABCI sockets: query, snapshot, mempool, and consensus.

use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_abci::SocketClient;
use eld_tendermint_consensus::SummaryQuery;
use eld_tendermint_crypto::sum;
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, RequestInfo,
    RequestInitChain, RequestQuery, ResponseBeginBlock, ResponseCheckTx, ResponseCommit,
    ResponseDeliverTx, ResponseEndBlock, ResponseInfo, ResponseInitChain, ResponseQuery,
};
use eld_tendermint_state::{Error as StateError, TM_CORE_SEMVER};
use eld_tendermint_types::BLOCK_PROTOCOL;

use crate::wait::{DeliveredTx, TxWaiter};

struct Shared {
    query: Mutex<SocketClient>,
    mempool: Mutex<SocketClient>,
    consensus: Mutex<SocketClient>,
    /// Kept open. Go dials this client even when state sync is off.
    #[allow(dead_code)]
    snapshot: Mutex<SocketClient>,
    waiter: Arc<TxWaiter>,
    /// Height from the latest `BeginBlock` header. `DeliverTx` reports this height.
    height: Mutex<i64>,
}

/// Query, mempool, and consensus sockets. Consensus and the mempool each hold a clone.
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
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                query: Mutex::new(query),
                snapshot: Mutex::new(snapshot),
                mempool: Mutex::new(mempool),
                consensus: Mutex::new(consensus),
                waiter: Arc::new(TxWaiter::new()),
                height: Mutex::new(0),
            }),
        }
    }

    pub(crate) fn waiter(&self) -> Arc<TxWaiter> {
        Arc::clone(&self.shared.waiter)
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

// Inbound content calls from the content reactor. This process has no blob
// store, so each call is refused and no reply is sent. The peer stays connected.
impl eld_tendermint_content::App for AbciApp {
    fn handle_content_push(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::ContentPush,
    ) -> Result<eld_tendermint_proto::content::ContentPushAck, eld_tendermint_content::Error> {
        refuse_content()
    }

    fn handle_content_request(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::ContentRequest,
    ) -> Result<Option<eld_tendermint_proto::content::ContentResponse>, eld_tendermint_content::Error>
    {
        refuse_content()
    }

    fn handle_capacity_challenge(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::CapacityChallenge,
    ) -> Result<
        eld_tendermint_proto::content::CapacityChallengeResponse,
        eld_tendermint_content::Error,
    > {
        refuse_content()
    }

    fn handle_content_push_ack(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::ContentPushAck,
    ) -> Result<(), eld_tendermint_content::Error> {
        refuse_content()
    }

    fn handle_content_response(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::ContentResponse,
    ) -> Result<(), eld_tendermint_content::Error> {
        refuse_content()
    }

    fn handle_capacity_challenge_response(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::CapacityChallengeResponse,
    ) -> Result<(), eld_tendermint_content::Error> {
        refuse_content()
    }

    fn handle_announce(
        &self,
        _peer_id: &str,
        _msg: eld_tendermint_proto::content::Announce,
    ) -> Result<(), eld_tendermint_content::Error> {
        refuse_content()
    }
}

fn refuse_content<T>() -> Result<T, eld_tendermint_content::Error> {
    Err(eld_tendermint_content::Error::Rejected {
        reason: "content is not stored in this process".to_owned(),
    })
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
