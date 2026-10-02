//! Flat `addrbook.json`. Same object as Go `addrBookJSON`, without bucket bias.

use std::fs;
use std::path::{Path, PathBuf};

use eld_tendermint_types::Time;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Serializer;
use serde_json::ser::PrettyFormatter;

use crate::Error;
use crate::address::NetAddress;

const BUCKET_NEW: u8 = 0x01;
const BUCKET_OLD: u8 = 0x02;

/// One persisted peer. `bucket_type` is `0x01` (new) or `0x02` (old).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownAddress {
    pub addr: NetAddress,
    pub src: NetAddress,
    pub buckets: Vec<i32>,
    pub attempts: i32,
    pub bucket_type: u8,
    pub last_attempt: Time,
    pub last_success: Time,
    pub last_ban_time: Time,
}

/// Addresses saved in `addrbook.json`.
#[derive(Clone, Debug)]
pub struct AddrBook {
    path: PathBuf,
    key: String,
    addrs: Vec<KnownAddress>,
}

impl AddrBook {
    /// Load `path`, or start empty when the file is missing.
    ///
    /// A file that does not decode as `addrBookJSON` panics, matching Go `loadFromFile`.
    #[must_use]
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        if !path.exists() {
            return Self {
                path,
                key: new_key(),
                addrs: Vec::new(),
            };
        }
        let bytes = fs::read(&path).unwrap_or_else(|err| {
            panic!("Error opening file {}: {err}", path.display());
        });
        let file: BookFile = serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            panic!("Error reading file {}: {err}", path.display());
        });
        let addrs = file
            .addrs
            .into_iter()
            .map(|known| known.into_known(&path))
            .collect();
        Self {
            path,
            key: file.key,
            addrs,
        }
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert `addr` when its id is not already stored. `bucket_type` is new.
    pub fn add(&mut self, addr: NetAddress, src: NetAddress) {
        if self.addrs.iter().any(|known| known.addr.id == addr.id) {
            return;
        }
        let now = Time::now();
        self.addrs.push(KnownAddress {
            addr,
            src,
            buckets: vec![0],
            attempts: 0,
            bucket_type: BUCKET_NEW,
            last_attempt: now,
            last_success: Time::GO_ZERO,
            last_ban_time: Time::GO_ZERO,
        });
    }

    /// `MarkGood`. Sets `bucket_type` to old and clears `attempts`.
    pub fn mark_good(&mut self, id: &str) {
        let Some(known) = self.addrs.iter_mut().find(|known| known.addr.id == id) else {
            return;
        };
        let now = Time::now();
        known.bucket_type = BUCKET_OLD;
        known.attempts = 0;
        known.last_attempt = now;
        known.last_success = now;
    }

    /// `MarkBad`. Drops the address from the list that [`Self::save`] writes.
    pub fn mark_bad(&mut self, addr: &NetAddress) {
        self.addrs.retain(|known| known.addr.id != addr.id);
    }

    /// First stored address whose id is not in `connected`.
    #[must_use]
    pub fn pick(&self, connected: &[String]) -> Option<NetAddress> {
        self.addrs
            .iter()
            .find(|known| !connected.iter().any(|id| id == &known.addr.id))
            .map(|known| known.addr.clone())
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&KnownAddress> {
        self.addrs.iter().find(|known| known.addr.id == id)
    }

    #[must_use]
    pub fn addresses(&self) -> Vec<NetAddress> {
        self.addrs.iter().map(|known| known.addr.clone()).collect()
    }

    /// Write `addrBookJSON` indented with tabs.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] when the file cannot be created.
    pub fn save(&self) -> Result<(), Error> {
        if let Some(parent) = self.path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(Error::Io)?;
            }
        }
        let file = BookFile::from_book(self);
        let mut buf = Vec::new();
        let formatter = PrettyFormatter::with_indent(b"\t");
        let mut ser = Serializer::with_formatter(&mut buf, formatter);
        file.serialize(&mut ser).expect("addrbook json serializes");
        buf.push(b'\n');
        fs::write(&self.path, buf).map_err(Error::Io)
    }
}

fn new_key() -> String {
    let mut bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

#[derive(Serialize, Deserialize)]
struct BookFile {
    key: String,
    addrs: Vec<KnownFile>,
}

impl BookFile {
    fn from_book(book: &AddrBook) -> Self {
        Self {
            key: book.key.clone(),
            addrs: book.addrs.iter().map(KnownFile::from_known).collect(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct KnownFile {
    addr: AddrFile,
    src: AddrFile,
    buckets: Vec<i32>,
    attempts: i32,
    bucket_type: u8,
    last_attempt: String,
    last_success: String,
    last_ban_time: String,
}

impl KnownFile {
    fn from_known(known: &KnownAddress) -> Self {
        Self {
            addr: AddrFile::from_net(&known.addr),
            src: AddrFile::from_net(&known.src),
            buckets: known.buckets.clone(),
            attempts: known.attempts,
            bucket_type: known.bucket_type,
            last_attempt: known.last_attempt.to_rfc3339(),
            last_success: known.last_success.to_rfc3339(),
            last_ban_time: known.last_ban_time.to_rfc3339(),
        }
    }

    fn into_known(self, path: &Path) -> KnownAddress {
        KnownAddress {
            addr: self.addr.into_net(path),
            src: self.src.into_net(path),
            buckets: self.buckets,
            attempts: self.attempts,
            bucket_type: self.bucket_type,
            last_attempt: parse_time(&self.last_attempt, path),
            last_success: parse_time(&self.last_success, path),
            last_ban_time: parse_time(&self.last_ban_time, path),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AddrFile {
    id: String,
    ip: String,
    port: u16,
}

impl AddrFile {
    fn from_net(addr: &NetAddress) -> Self {
        Self {
            id: addr.id.clone(),
            ip: addr.ip_json(),
            port: addr.port,
        }
    }

    fn into_net(self, path: &Path) -> NetAddress {
        let ip = NetAddress::ip_from_json(&self.ip).unwrap_or_else(|err| {
            panic!("Error reading file {}: {err}", path.display());
        });
        NetAddress {
            id: self.id.to_ascii_lowercase(),
            ip,
            port: self.port,
        }
    }
}

fn parse_time(text: &str, path: &Path) -> Time {
    Time::parse_rfc3339(text).unwrap_or_else(|_| {
        panic!("Error reading file {}: invalid time {text}", path.display());
    })
}
