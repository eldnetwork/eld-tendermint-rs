//! Go's four ABCI sockets: query, snapshot, mempool, and consensus.

use std::sync::{Arc, Mutex, MutexGuard};

use eld_tendermint_abci::SocketClient;
use eld_tendermint_crypto::sum;
use eld_tendermint_proto::abci::{
    RequestBeginBlock, RequestCheckTx, RequestDeliverTx, RequestEndBlock, RequestInitChain,
    RequestQuery, ResponseBeginBlock, ResponseCheckTx, ResponseCommit, ResponseDeliverTx,
    ResponseEndBlock, ResponseInitChain, ResponseQuery,
};
use eld_tendermint_state::Error as StateError;

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
