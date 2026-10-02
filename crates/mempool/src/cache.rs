//! `mempool.LRUTxCache` and `NopTxCache`.
//!
//! The key is `Tx.Key`: SHA-256 of the raw bytes, which is [`Tx::hash`](eld_tendermint_types::Tx::hash).

use std::collections::{HashSet, VecDeque};

use eld_tendermint_types::{Hash, Tx};

/// Seen-tx cache. `push` returns true when the tx was not already present.
pub(crate) enum TxCache {
    /// `NopTxCache`. `cache_size <= 0`.
    Nop,
    Lru {
        capacity: usize,
        keys: HashSet<Hash>,
        order: VecDeque<Hash>,
    },
}

impl TxCache {
    pub(crate) fn new(cache_size: i64) -> Self {
        let Ok(capacity) = usize::try_from(cache_size) else {
            return Self::Nop;
        };
        if capacity == 0 {
            return Self::Nop;
        }
        Self::Lru {
            capacity,
            keys: HashSet::new(),
            order: VecDeque::new(),
        }
    }

    /// `Push`. True when the tx is new. An existing key moves to the back.
    pub(crate) fn push(&mut self, tx: &Tx) -> bool {
        let Self::Lru {
            capacity,
            keys,
            order,
        } = self
        else {
            return true;
        };
        let key = tx.hash();
        if keys.contains(&key) {
            if let Some(index) = order.iter().position(|seen| *seen == key) {
                order.remove(index);
            }
            order.push_back(key);
            return false;
        }
        if order.len() >= *capacity {
            if let Some(oldest) = order.pop_front() {
                keys.remove(&oldest);
            }
        }
        keys.insert(key);
        order.push_back(key);
        true
    }

    pub(crate) fn remove(&mut self, tx: &Tx) {
        let Self::Lru { keys, order, .. } = self else {
            return;
        };
        let key = tx.hash();
        if keys.remove(&key) {
            if let Some(index) = order.iter().position(|seen| *seen == key) {
                order.remove(index);
            }
        }
    }
}
