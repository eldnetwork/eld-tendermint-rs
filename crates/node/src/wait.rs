//! Waiters for `DeliverTx` of one tx hash. There is no event bus.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard};

use eld_tendermint_proto::abci::ResponseDeliverTx;

/// One `DeliverTx` and the height `BeginBlock` recorded for that block.
#[derive(Clone)]
pub struct DeliveredTx {
    pub height: i64,
    pub response: ResponseDeliverTx,
}

struct Waiter {
    id: u64,
    sender: Sender<DeliveredTx>,
}

/// Fans a delivered tx out to the RPC calls waiting on its hash.
pub struct TxWaiter {
    next_id: Mutex<u64>,
    pending: Mutex<HashMap<[u8; 32], Vec<Waiter>>>,
}

/// A subscription created before `CheckTx` returns.
pub struct Subscription {
    hash: [u8; 32],
    id: u64,
    rx: Receiver<DeliveredTx>,
}

impl TxWaiter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register before `CheckTx` so a block that lands during the call is not missed.
    #[must_use]
    pub fn subscribe(&self, hash: [u8; 32]) -> Subscription {
        let (sender, rx) = channel();
        let mut ids = lock(&self.next_id);
        let id = *ids;
        *ids += 1;
        drop(ids);
        lock(&self.pending)
            .entry(hash)
            .or_default()
            .push(Waiter { id, sender });
        Subscription { hash, id, rx }
    }

    /// Drop this subscription. Other waiters for the same hash stay.
    pub fn cancel(&self, subscription: Subscription) {
        let mut pending = lock(&self.pending);
        if let Some(waiters) = pending.get_mut(&subscription.hash) {
            waiters.retain(|waiter| waiter.id != subscription.id);
            if waiters.is_empty() {
                pending.remove(&subscription.hash);
            }
        }
    }

    /// Wake every waiter for `hash`. `ApplyBlock`'s `DeliverTx` calls this.
    pub fn notify(&self, hash: [u8; 32], delivered: DeliveredTx) {
        let Some(waiters) = lock(&self.pending).remove(&hash) else {
            return;
        };
        for waiter in waiters {
            let _ = waiter.sender.send(delivered.clone());
        }
    }
}

impl Subscription {
    /// Block until [`TxWaiter::notify`] or `timeout`.
    ///
    /// # Errors
    ///
    /// Returns the channel error on timeout or when the waiter was removed.
    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<DeliveredTx, std::sync::mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
