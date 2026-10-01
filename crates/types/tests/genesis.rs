//! Genesis JSON cases from `types/genesis_test.go`.

use eld_tendermint_crypto::unmarshal_pub_key;
use eld_tendermint_types::{ConsensusParams, GenesisDoc, Time};

const PUB_JSON: &str = concat!(
    r#"{"type":"tendermint/PubKeyEd25519","value":""#,
    "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=",
    r#""}"#,
);

const GOOD: &str = r#"{
    "genesis_time": "2019-10-13T16:14:44Z",
    "chain_id": "test-chain-QDKdJr",
    "initial_height": "1000",
    "consensus_params": null,
    "validators": [{
        "pub_key":{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},
        "power":"10",
        "name":""
    }],
    "app_hash":"",
    "app_state":{"account_owner": "Bob"}
}"#;

#[test]
fn genesis_bad_json_is_rejected() {
    let too_long = "Lorem ipsum dolor sit amet, consectetuer adipiscing";
    let long_chain = format!(
        r#"{{"chain_id":"{too_long}","validators":[{{"pub_key":{{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="}},"power":"10","name":""}}]}}"#
    );
    let cases = [
        "",
        "not-json",
        "{}",
        r#"{"chain_id":"mychain","validators":[{}]}"#,
        r#"{"chain_id":"chain","initial_height":"-1"}"#,
        concat!(
            r#"{"validators":[{"pub_key":{"value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},"power":"10","name":""}]}"#,
        ),
        concat!(
            r#"{"validators":[{"pub_key":{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},"power":"10","name":""}]}"#,
        ),
        long_chain.as_str(),
        concat!(
            r#"{"chain_id":"mychain","validators":[{"address":"A","pub_key":{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},"power":"10","name":""}]}"#,
        ),
    ];
    for case in cases {
        assert!(GenesisDoc::from_json(case.as_bytes()).is_err(), "{case}");
    }
}

#[test]
fn genesis_good_json_fills_defaults_and_round_trips() {
    let doc = GenesisDoc::from_json(GOOD.as_bytes()).unwrap();
    assert_eq!(doc.chain_id.as_str(), "test-chain-QDKdJr");
    assert_eq!(doc.initial_height, 1000);
    assert_eq!(
        doc.genesis_time,
        Time::parse_rfc3339("2019-10-13T16:14:44Z").unwrap()
    );
    assert_eq!(
        doc.consensus_params,
        Some(ConsensusParams::default_params())
    );
    assert_eq!(doc.app_state.as_ref().unwrap()["account_owner"], "Bob");

    let key = unmarshal_pub_key(PUB_JSON).unwrap();
    assert_eq!(doc.validators[0].address, key.address().to_vec());
    assert_eq!(doc.validators[0].power, 10);
    assert_eq!(
        hex::encode(doc.validator_hash().unwrap().as_bytes()),
        "8c591e1c1b36b19bd29f16971023819e0c2ccaf634cdaa81ecdd30337dcc078d"
    );

    let expected = concat!(
        r#"{"genesis_time":"2019-10-13T16:14:44Z","chain_id":"test-chain-QDKdJr","initial_height":"1000","#,
        r#""consensus_params":{"block":{"max_bytes":"22020096","max_gas":"-1","time_iota_ms":"1000"},"#,
        r#""evidence":{"max_age_num_blocks":"100000","max_age_duration":"172800000000000","max_bytes":"1048576"},"#,
        r#""validator":{"pub_key_types":["ed25519"]},"version":{}},"validators":[{"address":"#,
        r#""A3258DCBF45DCA0DF052981870F2D1441A36D145","pub_key":{"type":"tendermint/PubKeyEd25519","value":"#,
        r#""AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},"power":"10","name":""}],"app_hash":"","#,
        r#""app_state":{"account_owner":"Bob"}}"#,
    );
    assert_eq!(doc.to_json(), expected);
    let again = GenesisDoc::from_json(doc.to_json().as_bytes()).unwrap();
    assert_eq!(again, doc);
}

#[test]
fn zero_genesis_time_is_replaced_and_missing_validators_are_allowed() {
    let doc = GenesisDoc::from_json(
        br#"{"genesis_time":"0001-01-01T00:00:00Z","chain_id":"mychain","consensus_params":null,"validators":[{"pub_key":{"type":"tendermint/PubKeyEd25519","value":"AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE="},"power":"10","name":""}],"app_hash":""}"#,
    )
    .unwrap();
    assert!(!doc.genesis_time.is_zero());
    assert_eq!(doc.initial_height, 1);

    for case in [
        br#"{"chain_id":"mychain"}"#.as_slice(),
        br#"{"chain_id":"mychain","validators":[]}"#,
        br#"{"chain_id":"mychain","validators":null}"#,
    ] {
        let parsed = GenesisDoc::from_json(case).unwrap();
        assert!(parsed.validators.is_empty());
        assert_eq!(parsed.initial_height, 1);
        assert!(parsed.consensus_params.is_some());
    }
}

#[test]
fn genesis_rejects_zero_block_max_bytes() {
    let mut doc = GenesisDoc::from_json(GOOD.as_bytes()).unwrap();
    doc.consensus_params.as_mut().unwrap().block.max_bytes = 0;
    assert!(GenesisDoc::from_json(doc.to_json().as_bytes()).is_err());
}
