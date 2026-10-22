//! `AppStateSnapshot` is the decompressed `SummaryBody.state_snapshot`.

use eld_tendermint_proto::types::{AppStateRecord, AppStateSnapshot};
use prost::Message;

#[test]
fn app_state_snapshot_round_trip() {
    let snapshot = AppStateSnapshot {
        block_height: 40,
        app_hash: vec![0xAB; 32],
        chain_id: "test-chain".to_string(),
        current_epoch: 2,
        state_trie_root: vec![0x11; 32],
        frontier: 17,
        records: vec![AppStateRecord {
            key: b"/@eld/account/0x01".to_vec(),
            value: vec![1, 2, 3],
        }],
        epoch_heights: vec![0, 2, 5],
        namespaces: vec![],
        validators: vec![],
        active_validators: vec![],
        capacity_validators: vec![],
        active_capacity_validator: None,
    };
    let bytes = snapshot.encode_to_vec();
    let decoded = AppStateSnapshot::decode(bytes.as_slice()).expect("decode");
    assert_eq!(decoded, snapshot);
    assert_eq!(decoded.frontier, 17);
    assert_eq!(decoded.records[0].value, vec![1, 2, 3]);
}
