//! `state/txindex/kv`. DeliverTx results in their own database.
//!
//! Keys match `kv.go`. There are no byte prefixes. The primary key is the raw
//! tmhash of the tx. The value is a protobuf `abci.TxResult`. Height and indexed
//! event attributes are secondary keys whose value is that hash.

use eld_tendermint_crypto::sum;
use eld_tendermint_proto::abci::{ResponseDeliverTx, TxResult};
use eld_tendermint_store::{Batch, Db};
use eld_tendermint_types::Block;
use prost::Message;
use prost::bytes::Bytes;

use crate::error::Error;

/// What consensus and fast sync call after `save_block`.
pub trait IndexTxs: Send + Sync {
    /// `AddBatch` for one committed block.
    ///
    /// # Errors
    ///
    /// Returns a database or decode error. A failure must not panic the process.
    fn index_committed(&self, block: &Block, results: &[ResponseDeliverTx]) -> Result<(), Error>;
}

/// KV tx index over a database that is not the block store.
pub struct TxIndex<D: Db> {
    db: D,
}

impl<D: Db> TxIndex<D> {
    #[must_use]
    pub fn new(db: D) -> Self {
        Self { db }
    }

    /// Store each DeliverTx. A later block with the same hash overwrites, matching
    /// `AddBatch` rather than the single-`Index` success guard.
    ///
    /// # Errors
    ///
    /// Returns when the result count does not match the block, or the batch write fails.
    pub fn index_committed(
        &self,
        block: &Block,
        results: &[ResponseDeliverTx],
    ) -> Result<(), Error> {
        let txs = block.data.as_slice();
        if txs.len() != results.len() {
            return Err(Error::Db(format!(
                "tx index: {} transactions and {} results",
                txs.len(),
                results.len()
            )));
        }
        let height = block.header.height;
        let mut batch = Batch::new();
        for (i, (tx, result)) in txs.iter().zip(results.iter()).enumerate() {
            let index = u32::try_from(i)
                .map_err(|_| Error::Db(format!("tx index {i} does not fit in uint32")))?;
            let hash = sum(tx.as_bytes());
            let stored = TxResult {
                height,
                index,
                tx: Bytes::copy_from_slice(tx.as_bytes()),
                result: Some(result.clone()),
            };
            batch.set(hash.as_slice(), &stored.encode_to_vec());
            batch.set(&key_for_height(height, index), hash.as_slice());
            for key in event_keys(result, height, index) {
                batch.set(&key, hash.as_slice());
            }
        }
        self.db
            .write_sync(&batch)
            .map_err(|err| Error::Db(err.to_string()))
    }

    /// `Get`. An empty hash is [`Error::EmptyTxHash`]. A missing hash is `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns a database error, or [`Error::BadTxResult`] when the value does not decode.
    pub fn get(&self, hash: &[u8]) -> Result<Option<TxResult>, Error> {
        if hash.is_empty() {
            return Err(Error::EmptyTxHash);
        }
        let Some(raw) = self
            .db
            .get(hash)
            .map_err(|err| Error::Db(err.to_string()))?
        else {
            return Ok(None);
        };
        TxResult::decode(raw.as_slice())
            .map(Some)
            .map_err(|err| Error::BadTxResult(err.to_string()))
    }

    /// Transactions whose height key is `tx.height/{height}/{height}/{index}`.
    ///
    /// The scan prefix is `tx.height/{height}/`, which does not include height 10
    /// when `height` is 1. Event keys are kept only when they are that height key.
    ///
    /// # Errors
    ///
    /// Returns a database or decode error. A height key whose hash is missing is an error.
    pub fn by_height(&self, height: i64) -> Result<Vec<TxResult>, Error> {
        let prefix = format!("tx.height/{height}/").into_bytes();
        let rows = self
            .db
            .iter_prefix(&prefix)
            .map_err(|err| Error::Db(err.to_string()))?;
        let mut found = Vec::new();
        for (key, hash) in rows {
            if !is_height_key(&key, height) {
                continue;
            }
            let Some(tx) = self.get(&hash)? else {
                return Err(Error::Db(format!(
                    "failed to get Tx{{{}}}",
                    hex_upper(&hash)
                )));
            };
            found.push(tx);
        }
        Ok(found)
    }
}

impl<D: Db + 'static> IndexTxs for TxIndex<D> {
    fn index_committed(&self, block: &Block, results: &[ResponseDeliverTx]) -> Result<(), Error> {
        TxIndex::index_committed(self, block, results)
    }
}

fn event_keys(result: &ResponseDeliverTx, height: i64, index: u32) -> Vec<Vec<u8>> {
    let mut keys = Vec::new();
    for event in &result.events {
        if event.r#type.is_empty() {
            continue;
        }
        for attr in &event.attributes {
            if attr.key.is_empty() || !attr.index {
                continue;
            }
            let mut composite = event.r#type.as_bytes().to_vec();
            composite.push(b'.');
            composite.extend_from_slice(&attr.key);
            keys.push(key_for_event(&composite, &attr.value, height, index));
        }
    }
    keys
}

/// `{type}.{key}/{value}/{height}/{index}`.
fn key_for_event(composite: &[u8], value: &[u8], height: i64, index: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(composite.len() + value.len() + 24);
    key.extend_from_slice(composite);
    key.push(b'/');
    key.extend_from_slice(value);
    key.push(b'/');
    key.extend_from_slice(height.to_string().as_bytes());
    key.push(b'/');
    key.extend_from_slice(index.to_string().as_bytes());
    key
}

/// `tx.height/{height}/{height}/{index}`. Height is written twice, as in `keyForHeight`.
fn key_for_height(height: i64, index: u32) -> Vec<u8> {
    format!("tx.height/{height}/{height}/{index}").into_bytes()
}

fn is_height_key(key: &[u8], height: i64) -> bool {
    let Ok(text) = std::str::from_utf8(key) else {
        return false;
    };
    let mut parts = text.split('/');
    if parts.next() != Some("tx.height") {
        return false;
    }
    let Some(first) = parts.next() else {
        return false;
    };
    let Some(second) = parts.next() else {
        return false;
    };
    let Some(index) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let height = height.to_string();
    first == height && second == height && index.parse::<u32>().is_ok()
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

#[cfg(test)]
mod tests {
    use eld_tendermint_proto::abci::{Event, EventAttribute, ResponseDeliverTx};
    use eld_tendermint_store::{Db, MemDb};
    use eld_tendermint_types::{Block, EvidenceList, Tx, Txs};
    use prost::bytes::Bytes;

    use super::{TxIndex, key_for_height};

    fn deliver(index_sender: bool) -> ResponseDeliverTx {
        ResponseDeliverTx {
            code: 7,
            log: "delivered".to_owned(),
            events: vec![
                Event {
                    r#type: "transfer".to_owned(),
                    attributes: vec![
                        EventAttribute {
                            key: Bytes::from_static(b"sender"),
                            value: Bytes::from_static(b"bob"),
                            index: index_sender,
                        },
                        EventAttribute {
                            key: Bytes::from_static(b"skip"),
                            value: Bytes::from_static(b"no"),
                            index: false,
                        },
                    ],
                },
                Event {
                    r#type: String::new(),
                    attributes: vec![EventAttribute {
                        key: Bytes::from_static(b"ignored"),
                        value: Bytes::from_static(b"x"),
                        index: true,
                    }],
                },
            ],
            ..ResponseDeliverTx::default()
        }
    }

    fn block_with(height: i64, raw: &[u8]) -> Block {
        Block::make_block(
            height,
            Txs::new(vec![Tx::new(raw.to_vec())]),
            None,
            EvidenceList::new(Vec::new()),
        )
    }

    #[test]
    fn hash_and_height_keys_round_trip() {
        let index = TxIndex::new(MemDb::new());
        let raw = b"pay";
        let block = block_with(1, raw);
        index
            .index_committed(&block, &[deliver(true)])
            .expect("index height 1");
        let hash = Tx::new(raw.to_vec()).hash();
        assert!(
            index.db.get(hash.as_bytes()).expect("read").is_some(),
            "hash key missing"
        );
        let got = index.get(hash.as_bytes()).expect("get").expect("tx");
        assert_eq!(got.height, 1);
        assert_eq!(got.index, 0);
        assert_eq!(got.tx.as_ref(), raw);
        let result = got.result.expect("result");
        assert_eq!(result.code, 7);
        assert_eq!(result.log, "delivered");

        let height_key = key_for_height(1, 0);
        assert_eq!(height_key, b"tx.height/1/1/0");
        let pointed = index
            .db
            .get(&height_key)
            .expect("read")
            .expect("height key");
        assert_eq!(pointed, hash.as_bytes());
        let event = index
            .db
            .get(b"transfer.sender/bob/1/0")
            .expect("read")
            .expect("event key");
        assert_eq!(event, hash.as_bytes());
        assert!(
            index
                .db
                .get(b"transfer.skip/no/1/0")
                .expect("read")
                .is_none()
        );

        let later = block_with(10, b"later");
        index
            .index_committed(&later, &[deliver(false)])
            .expect("index height 10");
        let at_one = index.by_height(1).expect("height 1");
        assert_eq!(at_one.len(), 1);
        assert_eq!(at_one[0].tx.as_ref(), raw);
        let at_ten = index.by_height(10).expect("height 10");
        assert_eq!(at_ten.len(), 1);
        assert_eq!(at_ten[0].tx.as_ref(), b"later");
        assert!(
            index
                .db
                .get(b"transfer.sender/bob/10/0")
                .expect("read")
                .is_none()
        );
    }
}
