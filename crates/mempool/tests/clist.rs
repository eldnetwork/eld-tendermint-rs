//! v0 mempool against an in-process app.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use eld_tendermint_config::MempoolConfig;
use eld_tendermint_proto::abci::{RequestCheckTx, ResponseCheckTx, ResponseDeliverTx};
use eld_tendermint_types::{Tx, Txs};
use prost::bytes::Bytes;

use eld_tendermint_mempool::{App, Error, Mempool};

struct Shared {
    check_count: u32,
    reject: HashSet<Vec<u8>>,
    gas_wanted: i64,
}

struct FakeApp {
    shared: Rc<RefCell<Shared>>,
}

impl App for FakeApp {
    fn check_tx(&mut self, request: RequestCheckTx) -> ResponseCheckTx {
        let mut shared = self.shared.borrow_mut();
        shared.check_count += 1;
        let code = u32::from(shared.reject.contains(request.tx.as_ref()));
        ResponseCheckTx {
            code,
            data: Bytes::new(),
            log: String::new(),
            info: String::new(),
            gas_wanted: shared.gas_wanted,
            gas_used: 0,
            events: Vec::new(),
            codespace: String::new(),
            sender: String::new(),
            priority: 0,
            mempool_error: String::new(),
        }
    }
}

fn pool(size: i64) -> (Mempool<FakeApp>, Rc<RefCell<Shared>>) {
    let shared = Rc::new(RefCell::new(Shared {
        check_count: 0,
        reject: HashSet::new(),
        gas_wanted: 1,
    }));
    let mut config = MempoolConfig::test_config();
    config.size = size;
    let mp = Mempool::new(
        config,
        FakeApp {
            shared: Rc::clone(&shared),
        },
    )
    .expect("v0 mempool");
    (mp, shared)
}

fn tx_byte(byte: u8) -> Tx {
    Tx::new(vec![byte])
}

fn tx_20(byte: u8) -> Tx {
    let mut bytes = vec![0u8; 20];
    bytes[0] = byte;
    Tx::new(bytes)
}

fn deliver_ok() -> ResponseDeliverTx {
    ResponseDeliverTx {
        code: 0,
        data: Bytes::new(),
        log: String::new(),
        info: String::new(),
        gas_wanted: 0,
        gas_used: 0,
        events: Vec::new(),
        codespace: String::new(),
    }
}

fn assert_txs(got: &Txs, expected: &[Tx]) {
    let got_bytes: Vec<&[u8]> = got.as_slice().iter().map(Tx::as_bytes).collect();
    let expected_bytes: Vec<&[u8]> = expected.iter().map(Tx::as_bytes).collect();
    if got_bytes != expected_bytes {
        panic!("reaped {got_bytes:?}\n!= {expected_bytes:?}");
    }
}

#[test]
fn five_txs_reap_in_insertion_order() {
    let (mut mp, _) = pool(5000);
    let txs: Vec<Tx> = (1..=5).map(tx_byte).collect();
    for tx in &txs {
        mp.check_tx(tx).expect("check");
    }
    let got = mp.reap_max_bytes_max_gas(-1, -1);
    assert_txs(&got, &txs);
}

#[test]
fn duplicate_tx_is_rejected() {
    let (mut mp, _) = pool(5000);
    let tx = tx_byte(1);
    mp.check_tx(&tx).expect("first");
    let err = mp.check_tx(&tx).expect_err("duplicate");
    assert!(matches!(err, Error::TxInCache), "{err}");
    assert_eq!(mp.size(), 1);
}

#[test]
fn non_zero_code_does_not_occupy_a_slot() {
    let (mut mp, shared) = pool(5000);
    let tx = tx_byte(7);
    shared.borrow_mut().reject.insert(tx.as_bytes().to_vec());
    mp.check_tx(&tx)
        .expect("rejected code is not a check error");
    assert_eq!(mp.size(), 0);
    shared.borrow_mut().reject.clear();
    mp.check_tx(&tx).expect("retry after invalid");
    assert_eq!(mp.size(), 1);
}

#[test]
fn full_mempool_rejects_before_the_app() {
    let (mut mp, shared) = pool(1);
    mp.check_tx(&tx_byte(1)).expect("first");
    let checks = shared.borrow().check_count;
    let err = mp.check_tx(&tx_byte(2)).expect_err("full");
    assert!(err.to_string().contains("mempool is full"), "{err}");
    assert_eq!(mp.size(), 1);
    assert_eq!(shared.borrow().check_count, checks);
}

#[test]
fn recheck_drops_a_tx_the_app_now_rejects() {
    let (mut mp, shared) = pool(5000);
    let first = tx_byte(1);
    let second = tx_byte(2);
    mp.check_tx(&first).expect("first");
    mp.check_tx(&second).expect("second");
    shared
        .borrow_mut()
        .reject
        .insert(second.as_bytes().to_vec());
    mp.update(1, &[first], &[deliver_ok()], None, None)
        .expect("update");
    assert_eq!(mp.size(), 0);
    assert_txs(&mp.reap_max_bytes_max_gas(-1, -1), &[]);
}

#[test]
fn reap_stops_on_bytes_and_on_zero_gas() {
    let (mut mp, _) = pool(5000);
    let first = tx_20(1);
    let second = tx_20(2);
    mp.check_tx(&first).expect("first");
    mp.check_tx(&second).expect("second");
    let by_bytes = mp.reap_max_bytes_max_gas(24, -1);
    assert_txs(&by_bytes, &[first]);
    let by_gas = mp.reap_max_bytes_max_gas(-1, 0);
    assert_txs(&by_gas, &[]);
}
