//! `tm-db` surface used by the block store.
//!
//! `get` of a missing key is `None`. An empty value is also missing, matching Go's
//! `len(bz) == 0`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use rocksdb::{DB, Direction, IteratorMode, WriteOptions};

use crate::error::Error;

/// One key and its value from [`Db::iter_prefix`].
pub type PrefixRow = (Vec<u8>, Vec<u8>);

/// One queued `set` or `delete`. Applied in order by [`Db::write_sync`].
#[derive(Clone, Debug)]
enum BatchOp {
    Set(Vec<u8>, Vec<u8>),
    Delete(Vec<u8>),
}

/// `dbm.Batch`. `save_block` does not use it. Prune will.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    ops: Vec<BatchOp>,
}

impl Batch {
    #[must_use]
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    pub fn set(&mut self, key: &[u8], value: &[u8]) {
        self.ops.push(BatchOp::Set(key.to_vec(), value.to_vec()));
    }

    pub fn delete(&mut self, key: &[u8]) {
        self.ops.push(BatchOp::Delete(key.to_vec()));
    }
}

/// Key-value store. `BlockStore` takes `D: Db`, including `Arc<MemDb>`.
pub trait Db: Send + Sync {
    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend read fails. `MemDb` does not fail.
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Error>;

    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend write fails.
    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), Error>;

    /// `SetSync`. Flushes before returning. `MemDb` is the same as [`Self::set`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend write fails.
    fn set_sync(&self, key: &[u8], value: &[u8]) -> Result<(), Error>;

    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend delete fails.
    fn delete(&self, key: &[u8]) -> Result<(), Error>;

    /// `Batch.WriteSync`. Applies [`Batch::set`] and [`Batch::delete`] in order.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend write fails.
    fn write_sync(&self, batch: &Batch) -> Result<(), Error>;

    /// Keys with this prefix, in ascending order, including empty values.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Db`] when the backend scan fails.
    fn iter_prefix(&self, prefix: &[u8]) -> Result<Vec<PrefixRow>, Error>;
}

impl<T: Db + ?Sized> Db for Arc<T> {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        (**self).get(key)
    }

    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        (**self).set(key, value)
    }

    fn set_sync(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        (**self).set_sync(key, value)
    }

    fn delete(&self, key: &[u8]) -> Result<(), Error> {
        (**self).delete(key)
    }

    fn write_sync(&self, batch: &Batch) -> Result<(), Error> {
        (**self).write_sync(batch)
    }

    fn iter_prefix(&self, prefix: &[u8]) -> Result<Vec<PrefixRow>, Error> {
        (**self).iter_prefix(prefix)
    }
}

/// In-memory `dbm.MemDB`, backed by a `BTreeMap`.
#[derive(Debug, Default)]
pub struct MemDb {
    map: Mutex<BTreeMap<Vec<u8>, Vec<u8>>>,
}

impl MemDb {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Db for MemDb {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let map = self.map.lock().expect("memdb lock");
        Ok(map.get(key).filter(|value| !value.is_empty()).cloned())
    }

    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        let mut map = self.map.lock().expect("memdb lock");
        map.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn set_sync(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.set(key, value)
    }

    fn delete(&self, key: &[u8]) -> Result<(), Error> {
        let mut map = self.map.lock().expect("memdb lock");
        map.remove(key);
        Ok(())
    }

    fn write_sync(&self, batch: &Batch) -> Result<(), Error> {
        let mut map = self.map.lock().expect("memdb lock");
        apply_batch(&mut map, batch);
        Ok(())
    }

    fn iter_prefix(&self, prefix: &[u8]) -> Result<Vec<PrefixRow>, Error> {
        let map = self.map.lock().expect("memdb lock");
        let rows = match prefix_end(prefix) {
            Some(end) => map
                .range(prefix.to_vec()..end)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            None => map
                .range(prefix.to_vec()..)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        };
        Ok(rows)
    }
}

fn apply_batch(map: &mut BTreeMap<Vec<u8>, Vec<u8>>, batch: &Batch) {
    for op in &batch.ops {
        match op {
            BatchOp::Set(key, value) => {
                map.insert(key.clone(), value.clone());
            }
            BatchOp::Delete(key) => {
                map.remove(key);
            }
        }
    }
}

/// Exclusive end of the key range that starts with `prefix`.
/// `None` means every key greater than or equal to `prefix`.
fn prefix_end(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.last_mut() {
        if *last != u8::MAX {
            *last += 1;
            return Some(end);
        }
        end.pop();
    }
    None
}

/// RocksDB in a directory this crate owns. One default column family.
///
/// Opening a Go goleveldb directory returns [`Error::GoLevelDb`] and does not
/// create RocksDB files there. goleveldb writes `CURRENT`, `LOG`, and `*.ldb`,
/// and never `IDENTITY`. RocksDB writes `IDENTITY` on open, so reopening our
/// own directory succeeds.
pub struct RocksDb {
    db: DB,
}

impl RocksDb {
    /// # Errors
    ///
    /// Returns [`Error::GoLevelDb`] for a goleveldb layout, [`Error::Io`] when
    /// the path exists and is not a directory, and [`Error::Db`] when RocksDB
    /// fails to open.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        refuse_goleveldb(path)?;
        let db = DB::open_default(path).map_err(|err| Error::Db(err.to_string()))?;
        Ok(Self { db })
    }
}

fn refuse_goleveldb(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        return Ok(());
    }
    let meta = fs::metadata(path).map_err(|err| Error::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    if !meta.is_dir() {
        return Err(Error::Io {
            path: path.to_path_buf(),
            message: "not a directory".to_owned(),
        });
    }
    let mut has_current = false;
    let mut has_log = false;
    let mut has_identity = false;
    let mut has_ldb = false;
    for entry in fs::read_dir(path).map_err(|err| Error::Io {
        path: path.to_path_buf(),
        message: err.to_string(),
    })? {
        let entry = entry.map_err(|err| Error::Io {
            path: path.to_path_buf(),
            message: err.to_string(),
        })?;
        let name = entry.file_name();
        if name == "CURRENT" {
            has_current = true;
        } else if name == "LOG" {
            has_log = true;
        } else if name == "IDENTITY" {
            has_identity = true;
        }
        if name.to_string_lossy().ends_with(".ldb") {
            has_ldb = true;
        }
    }
    if has_ldb || (has_current && has_log && !has_identity) {
        return Err(Error::GoLevelDb {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn sync_options() -> WriteOptions {
    let mut options = WriteOptions::default();
    options.set_sync(true);
    options
}

impl Db for RocksDb {
    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
        let value = self.db.get(key).map_err(|err| Error::Db(err.to_string()))?;
        Ok(value.filter(|bytes| !bytes.is_empty()))
    }

    fn set(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.db
            .put(key, value)
            .map_err(|err| Error::Db(err.to_string()))
    }

    fn set_sync(&self, key: &[u8], value: &[u8]) -> Result<(), Error> {
        self.db
            .put_opt(key, value, &sync_options())
            .map_err(|err| Error::Db(err.to_string()))
    }

    fn delete(&self, key: &[u8]) -> Result<(), Error> {
        self.db
            .delete(key)
            .map_err(|err| Error::Db(err.to_string()))
    }

    fn write_sync(&self, batch: &Batch) -> Result<(), Error> {
        let mut write_batch = rocksdb::WriteBatch::default();
        for op in &batch.ops {
            match op {
                BatchOp::Set(key, value) => write_batch.put(key, value),
                BatchOp::Delete(key) => write_batch.delete(key),
            }
        }
        self.db
            .write_opt(write_batch, &sync_options())
            .map_err(|err| Error::Db(err.to_string()))
    }

    fn iter_prefix(&self, prefix: &[u8]) -> Result<Vec<PrefixRow>, Error> {
        let mode = if prefix.is_empty() {
            IteratorMode::Start
        } else {
            IteratorMode::From(prefix, Direction::Forward)
        };
        let mut rows = Vec::new();
        for item in self.db.iterator(mode) {
            let (key, value) = item.map_err(|err| Error::Db(err.to_string()))?;
            if !prefix.is_empty() && !key.starts_with(prefix) {
                break;
            }
            rows.push((key.to_vec(), value.to_vec()));
        }
        Ok(rows)
    }
}
