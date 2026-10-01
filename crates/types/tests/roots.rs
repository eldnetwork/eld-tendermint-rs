//! Transaction roots, validator-set hash, and consensus-params hash from the Go tree.

use eld_tendermint_crypto::{PubKey, unmarshal_pub_key};
use eld_tendermint_types::{
    ConsensusParams, Tx, Txs, Validator, ValidatorSet, hash_consensus_params,
};

const PUB_JSON: &str = concat!(
    r#"{"type":"tendermint/PubKeyEd25519","value":""#,
    "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=",
    r#""}"#,
);

fn pub_key() -> PubKey {
    unmarshal_pub_key(PUB_JSON).unwrap()
}

#[test]
fn tx_roots_match_go() {
    let cases = [
        (
            vec![Tx::new(vec![1, 4, 34, 87, 163, 1])],
            "2f19a1abe8df582ffa30d9d49da8f0251553fd9e318b9c6a72ba23ebc747e499",
        ),
        (
            vec![Tx::new(vec![5, 56, 165, 2]), Tx::new(vec![4, 77])],
            "f13cce25ca349132985b5359212b3842216d41dfa437334fbd5d6218b195bbf1",
        ),
        (
            vec![
                Tx::new(b"foo".to_vec()),
                Tx::new(b"bar".to_vec()),
                Tx::new(b"baz".to_vec()),
            ],
            "c9148115f7280f200540da16817e48993037810e4592366390aa81258546107d",
        ),
    ];
    for (txs, want) in cases {
        assert_eq!(hex::encode(Txs::new(txs).hash().as_bytes()), want);
    }
}

#[test]
fn empty_txs_hash_is_empty_tree() {
    assert_eq!(
        hex::encode(Txs::default().hash().as_bytes()),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn validator_set_hash_matches_go() {
    let set = ValidatorSet::new(vec![Validator::new(pub_key(), 10)]).unwrap();
    assert_eq!(
        hex::encode(set.hash().unwrap().as_bytes()),
        "8c591e1c1b36b19bd29f16971023819e0c2ccaf634cdaa81ecdd30337dcc078d"
    );
    assert_eq!(set.total_voting_power(), 10);
    assert_eq!(set.proposer().unwrap().voting_power, 10);
    assert!(set.validate_basic().is_ok());
}

#[test]
fn validator_set_sorts_by_power_then_address_and_rejects_bad_sets() {
    let low = Validator::new(pub_key(), 1);
    let mut high_key = [0u8; 32];
    high_key[31] = 1;
    let high = Validator::new(PubKey::from_bytes(&high_key).unwrap(), 5);
    let set = ValidatorSet::new(vec![low.clone(), high.clone()]).unwrap();
    assert_eq!(set.validators()[0].voting_power, 5);
    assert_eq!(set.validators()[1].voting_power, 1);

    let proposer = set.proposer().unwrap();
    assert_eq!(proposer.voting_power, 5);
    assert_eq!(proposer.address, high.address);

    assert!(ValidatorSet::new(vec![]).is_err());
    assert!(ValidatorSet::default().validate_basic().is_err());
    assert!(ValidatorSet::new(vec![Validator::new(pub_key(), 0)]).is_err());
    assert!(ValidatorSet::new(vec![low.clone(), low]).is_err());

    let mut negative = Validator::new(pub_key(), 1);
    negative.voting_power = -1;
    assert!(negative.validate_basic().is_err());
    negative.voting_power = 1;
    negative.pub_key = None;
    assert!(negative.validate_basic().is_err());
    negative.pub_key = Some(pub_key());
    negative.address = vec![1];
    assert!(negative.validate_basic().is_err());
}

#[test]
fn default_consensus_params_hash_matches_go() {
    let params = ConsensusParams::default_params();
    assert!(params.validate().is_ok());
    assert_eq!(
        hex::encode(hash_consensus_params(&params).as_bytes()),
        "048091bc7ddc283f77bfbf91d73c44da58c3df8a9cbc867405d8b7f3daada22f"
    );

    let mut zero_bytes = params.clone();
    zero_bytes.block.max_bytes = 0;
    assert!(zero_bytes.validate().is_err());
}
