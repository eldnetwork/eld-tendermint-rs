//! Waiters for a content ack or blob. The RPC call registers before the send.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, MutexGuard};

/// A push receipt or a fetched blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ContentReply {
    Ack { provider: Vec<u8> },
    Blob { bytes: Vec<u8> },
}

struct Waiter {
    id: u64,
    sender: Sender<ContentReply>,
}

/// Fans one content reply out to the RPC calls waiting on its request id.
pub(crate) struct ContentWaiter {
    next_id: Mutex<u64>,
    pending: Mutex<HashMap<u64, Vec<Waiter>>>,
}

pub(crate) struct ContentSubscription {
    request_id: u64,
    id: u64,
    rx: Receiver<ContentReply>,
}

impl ContentWaiter {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// Register before the content message is queued.
    #[must_use]
    pub(crate) fn subscribe(&self, request_id: u64) -> ContentSubscription {
        let (sender, rx) = channel();
        let mut ids = lock(&self.next_id);
        let id = *ids;
        *ids += 1;
        drop(ids);
        lock(&self.pending)
            .entry(request_id)
            .or_default()
            .push(Waiter { id, sender });
        ContentSubscription { request_id, id, rx }
    }

    pub(crate) fn cancel(&self, subscription: &ContentSubscription) {
        let mut pending = lock(&self.pending);
        if let Some(waiters) = pending.get_mut(&subscription.request_id) {
            waiters.retain(|waiter| waiter.id != subscription.id);
            if waiters.is_empty() {
                pending.remove(&subscription.request_id);
            }
        }
    }

    pub(crate) fn notify(&self, request_id: u64, reply: ContentReply) {
        let Some(waiters) = lock(&self.pending).remove(&request_id) else {
            return;
        };
        for waiter in waiters {
            let _ = waiter.sender.send(reply.clone());
        }
    }
}

impl ContentSubscription {
    pub(crate) fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<ContentReply, std::sync::mpsc::RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ack_wakes_the_waiter_for_that_request() {
        let waiter = ContentWaiter::new();
        let subscription = waiter.subscribe(7);
        waiter.notify(
            7,
            ContentReply::Ack {
                provider: vec![1, 2, 3],
            },
        );
        let reply = subscription
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("ack");
        assert_eq!(
            reply,
            ContentReply::Ack {
                provider: vec![1, 2, 3],
            }
        );
    }
}
