//! v0 `CListMempool`. FIFO. CheckTx, reap, and recheck are synchronous.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use eld_tendermint_config::{MEMPOOL_V0, MempoolConfig};
use eld_tendermint_proto::abci::{CheckTxType, RequestCheckTx, ResponseCheckTx, ResponseDeliverTx};
use eld_tendermint_types::{Level, Tx, Txs, log_line, upper_hex};
use prost::Message;
use prost::bytes::Bytes;

use crate::cache::TxCache;
use crate::error::Error;

/// Filter run before `CheckTx`. `Err` rejects the tx and the app is not called.
pub type PreCheck = Box<dyn Fn(&Tx) -> Result<(), String> + Send>;

/// Filter run after a successful `CheckTx`. `Err` keeps the tx out of the pool.
pub type PostCheck = Box<dyn Fn(&Tx, &ResponseCheckTx) -> Result<(), String> + Send>;

/// In-process mempool ABCI. Not the socket client.
pub trait App {
    fn check_tx(&mut self, request: RequestCheckTx) -> ResponseCheckTx;
}

struct Entry {
    gas_wanted: i64,
    tx: Tx,
    senders: HashSet<u16>,
}

/// Ordered pool of transactions that passed `CheckTx`.
pub struct Mempool<A: App> {
    config: MempoolConfig,
    app: A,
    txs: VecDeque<Entry>,
    txs_bytes: i64,
    cache: TxCache,
    pre_check: Option<PreCheck>,
    post_check: Option<PostCheck>,
    txs_available: Option<SyncSender<()>>,
    notified_txs_available: bool,
}

impl<A: App> Mempool<A> {
    /// `NewCListMempool` for `version = "v0"`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnsupportedVersion`] when `config.version` is not `v0`.
    pub fn new(config: MempoolConfig, app: A) -> Result<Self, Error> {
        if config.version != MEMPOOL_V0 {
            return Err(Error::UnsupportedVersion {
                got: config.version.clone(),
            });
        }
        let cache = TxCache::new(config.cache_size);
        Ok(Self {
            config,
            app,
            txs: VecDeque::new(),
            txs_bytes: 0,
            cache,
            pre_check: None,
            post_check: None,
            txs_available: None,
            notified_txs_available: false,
        })
    }

    /// Number of txs in the pool.
    #[must_use]
    pub fn size(&self) -> usize {
        self.txs.len()
    }

    /// Sum of raw tx lengths.
    #[must_use]
    pub fn size_bytes(&self) -> i64 {
        self.txs_bytes
    }

    /// `EnableTxsAvailable`. The channel fires once per height, after the pool
    /// becomes non-empty, until the next [`Self::update`].
    pub fn enable_txs_available(&mut self) -> Receiver<()> {
        let (sender, receiver) = sync_channel(1);
        self.txs_available = Some(sender);
        self.notified_txs_available = false;
        receiver
    }

    /// `CheckTx`. A non-zero ABCI code does not occupy a slot and returns `Ok`.
    /// Fullness, size, the cache, and `pre_check` return `Err` before the app runs,
    /// except the second fullness check after a successful response.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MempoolIsFull`], [`Error::TxTooLarge`], [`Error::TxInCache`],
    /// or [`Error::PreCheck`].
    pub fn check_tx(&mut self, tx: &Tx) -> Result<(), Error> {
        self.check_tx_response(tx).map(|_| ())
    }

    /// `CheckTx` returning the ABCI response. A non-zero code is `Ok` and is not pooled.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::check_tx`].
    pub fn check_tx_response(&mut self, tx: &Tx) -> Result<ResponseCheckTx, Error> {
        self.check_tx_inner(tx, CheckTxType::New, 0)
    }

    /// `CheckTx` recording `sender_id` so the reactor does not gossip the tx back.
    ///
    /// `0` is the local sender. A cache hit on a tx still in the pool adds
    /// `sender_id` and returns [`Error::TxInCache`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::check_tx`].
    pub fn check_tx_with_sender(&mut self, tx: &Tx, sender_id: u16) -> Result<(), Error> {
        self.check_tx_inner(tx, CheckTxType::New, sender_id)
            .map(|_| ())
    }

    pub(crate) fn broadcasts(&self) -> bool {
        self.config.broadcast
    }

    /// Pooled txs in arrival order, each with the peer ids that have sent it.
    pub(crate) fn pooled(&self) -> Vec<(Tx, HashSet<u16>)> {
        self.txs
            .iter()
            .map(|entry| (entry.tx.clone(), entry.senders.clone()))
            .collect()
    }

    fn check_tx_inner(
        &mut self,
        tx: &Tx,
        kind: CheckTxType,
        sender_id: u16,
    ) -> Result<ResponseCheckTx, Error> {
        let tx_size = i64::try_from(tx.as_bytes().len()).unwrap_or(i64::MAX);
        self.is_full(tx_size)?;
        if tx_size > self.config.max_tx_bytes {
            return Err(Error::TxTooLarge {
                max: self.config.max_tx_bytes,
                actual: tx_size,
            });
        }
        if let Some(pre_check) = &self.pre_check {
            if let Err(reason) = pre_check(tx) {
                return Err(Error::PreCheck { reason });
            }
        }
        if !self.cache.push(tx) {
            self.note_sender(tx, sender_id);
            return Err(Error::TxInCache);
        }
        let response = self.app.check_tx(RequestCheckTx {
            tx: Bytes::copy_from_slice(tx.as_bytes()),
            r#type: kind as i32,
        });
        let post_err = self
            .post_check
            .as_ref()
            .and_then(|post| post(tx, &response).err());
        if response.code != 0 {
            let code = response.code.to_string();
            let hash = upper_hex(tx.hash().as_bytes());
            log_line(
                Level::Error,
                "mempool",
                "CheckTx failed",
                &[("code", code.as_str()), ("hash", hash.as_str())],
            );
        }
        if response.code == 0 && post_err.is_none() {
            if let Err(err) = self.is_full(tx_size) {
                self.cache.remove(tx);
                let _ = err;
                return Ok(response);
            }
            self.txs_bytes += tx_size;
            let mut senders = HashSet::new();
            senders.insert(sender_id);
            self.txs.push_back(Entry {
                gas_wanted: response.gas_wanted,
                tx: tx.clone(),
                senders,
            });
            self.notify_txs_available();
            return Ok(response);
        }
        if !self.config.keep_invalid_txs_in_cache {
            self.cache.remove(tx);
        }
        Ok(response)
    }

    /// `ReapMaxBytesMaxGas`. Insertion order. Does not remove txs.
    ///
    /// `max_bytes < 0` and `max_gas < 0` return every tx. A tx's byte size is the
    /// protobuf length of `Data` containing that tx alone.
    #[must_use]
    pub fn reap_max_bytes_max_gas(&self, max_bytes: i64, max_gas: i64) -> Txs {
        let mut total_gas = 0i64;
        let mut running_size = 0i64;
        let mut out = Vec::new();
        for entry in &self.txs {
            out.push(entry.tx.clone());
            let data_size = proto_size(&entry.tx);
            if max_bytes > -1 && running_size.saturating_add(data_size) > max_bytes {
                out.pop();
                return Txs::new(out);
            }
            running_size = running_size.saturating_add(data_size);
            let new_total_gas = total_gas.wrapping_add(entry.gas_wanted);
            if max_gas > -1 && new_total_gas > max_gas {
                out.pop();
                return Txs::new(out);
            }
            total_gas = new_total_gas;
        }
        Txs::new(out)
    }

    /// `Update`. Removes committed txs, then rechecks the rest when `recheck` is set.
    ///
    /// `new_pre_check` and `new_post_check` replace the current filters when present.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MismatchedDeliverResponses`] when the slices differ in length.
    pub fn update(
        &mut self,
        height: i64,
        block_txs: &[Tx],
        deliver_responses: &[ResponseDeliverTx],
        new_pre_check: Option<PreCheck>,
        new_post_check: Option<PostCheck>,
    ) -> Result<(), Error> {
        if block_txs.len() != deliver_responses.len() {
            return Err(Error::MismatchedDeliverResponses {
                txs: block_txs.len(),
                responses: deliver_responses.len(),
            });
        }
        self.notified_txs_available = false;
        if let Some(pre_check) = new_pre_check {
            self.pre_check = Some(pre_check);
        }
        if let Some(post_check) = new_post_check {
            self.post_check = Some(post_check);
        }
        for (tx, response) in block_txs.iter().zip(deliver_responses.iter()) {
            if response.code == 0 {
                let _ = self.cache.push(tx);
            } else {
                let code = response.code.to_string();
                let height = height.to_string();
                let hash = upper_hex(tx.hash().as_bytes());
                log_line(
                    Level::Error,
                    "mempool",
                    "DeliverTx failed",
                    &[
                        ("code", code.as_str()),
                        ("height", height.as_str()),
                        ("hash", hash.as_str()),
                    ],
                );
                if !self.config.keep_invalid_txs_in_cache {
                    self.cache.remove(tx);
                }
            }
            self.remove_tx(tx, false);
        }
        if self.txs.is_empty() {
            return Ok(());
        }
        if self.config.recheck {
            self.recheck();
        } else {
            self.notify_txs_available();
        }
        Ok(())
    }

    fn recheck(&mut self) {
        let pending: Vec<Tx> = self.txs.iter().map(|entry| entry.tx.clone()).collect();
        for tx in pending {
            let response = self.app.check_tx(RequestCheckTx {
                tx: Bytes::copy_from_slice(tx.as_bytes()),
                r#type: CheckTxType::Recheck as i32,
            });
            let post_err = self
                .post_check
                .as_ref()
                .and_then(|post| post(&tx, &response).err());
            if response.code != 0 || post_err.is_some() {
                self.remove_tx(&tx, !self.config.keep_invalid_txs_in_cache);
            }
        }
        if !self.txs.is_empty() {
            self.notify_txs_available();
        }
    }

    fn note_sender(&mut self, tx: &Tx, sender_id: u16) {
        if let Some(entry) = self.txs.iter_mut().find(|entry| entry.tx == *tx) {
            entry.senders.insert(sender_id);
        }
    }

    fn remove_tx(&mut self, tx: &Tx, remove_from_cache: bool) {
        let Some(index) = self.txs.iter().position(|entry| entry.tx == *tx) else {
            return;
        };
        let removed = self
            .txs
            .remove(index)
            .unwrap_or_else(|| unreachable!("index was just found"));
        let len = i64::try_from(removed.tx.as_bytes().len()).unwrap_or(i64::MAX);
        self.txs_bytes -= len;
        if remove_from_cache {
            self.cache.remove(tx);
        }
    }

    fn is_full(&self, tx_size: i64) -> Result<(), Error> {
        let num_txs = i64::try_from(self.txs.len()).unwrap_or(i64::MAX);
        if num_txs >= self.config.size
            || tx_size.saturating_add(self.txs_bytes) > self.config.max_txs_bytes
        {
            return Err(Error::MempoolIsFull {
                num_txs,
                max_txs: self.config.size,
                txs_bytes: self.txs_bytes,
                max_txs_bytes: self.config.max_txs_bytes,
            });
        }
        Ok(())
    }

    fn notify_txs_available(&mut self) {
        if self.txs.is_empty() {
            return;
        }
        let Some(sender) = &self.txs_available else {
            return;
        };
        if self.notified_txs_available {
            return;
        }
        self.notified_txs_available = true;
        let _ = sender.try_send(());
    }
}

/// `ComputeProtoSizeForTxs` of a one-element list.
fn proto_size(tx: &Tx) -> i64 {
    let data = eld_tendermint_proto::types::Data {
        txs: vec![tx.as_bytes().to_vec()],
    };
    i64::try_from(data.encoded_len()).unwrap_or(i64::MAX)
}
