#![allow(clippy::unwrap_used, clippy::expect_used, clippy::dbg_macro)]
//! Summary header and body hashes.

use prost::Message;

use eld_tendermint_crypto::{PubKey, sum, unmarshal_pub_key};
use eld_tendermint_types::{
    BlockId, ConsensusParams, Error, GenesisDoc, Hash, PartSetHeader, SummaryBody, SummaryHeader,
    SummaryInput, Validator, ValidatorSet, genesis_anchor_hash,
};

const PUB_JSON: &str = concat!(
    r#"{"type":"tendermint/PubKeyEd25519","value":""#,
    "AT/+aaL1eB0477Mud9JMm8Sh8BIvOYlPGC9KkIUmFaE=",
    r#""}"#,
);

const GENESIS: &str = r#"{
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

const BODY: &[u8] = b"compressed-snapshot";

fn pub_key() -> PubKey {
    unmarshal_pub_key(PUB_JSON).unwrap()
}

fn block_id() -> BlockId {
    BlockId {
        hash: sum(b"block").to_vec(),
        part_set_header: PartSetHeader {
            total: 1,
            hash: sum(b"parts").to_vec(),
        },
    }
}

fn header_for(body: &[u8], prev_summary_hash: Hash) -> SummaryHeader {
    let validators = ValidatorSet::new(vec![Validator::new(pub_key(), 10)]).unwrap();
    let mut next_validators = validators.copy();
    next_validators.increment_proposer_priority(1).unwrap();
    SummaryHeader::build(
        SummaryInput {
            end_height: 10,
            prev_summary_hash,
            last_block_id: block_id(),
            last_results_hash: Hash::from_array(sum(b"results")),
            app_hash: Hash::from_array(sum(b"app")),
            last_height_validators_changed: 1,
            last_height_consensus_params_changed: 1,
            validators,
            next_validators,
            consensus_params: ConsensusParams::default_params(),
        },
        body,
    )
    .unwrap()
}

fn sample_header() -> SummaryHeader {
    header_for(BODY, Hash::from_array(sum(b"prev-summary")))
}

#[test]
fn summary_prost_round_trip() {
    let header = sample_header();
    let proto = header.to_proto().unwrap();
    let decoded =
        eld_tendermint_proto::types::SummaryHeader::decode(proto.encode_to_vec().as_slice())
            .unwrap();
    let again = SummaryHeader::try_from_proto(&decoded).unwrap();
    assert_eq!(again, header);

    let body = SummaryBody {
        state_snapshot: BODY.to_vec(),
    };
    assert_eq!(header.body_hash, body.hash());
    let body_proto = body.to_proto();
    let body_decoded =
        eld_tendermint_proto::types::SummaryBody::decode(body_proto.encode_to_vec().as_slice())
            .unwrap();
    assert_eq!(SummaryBody::from_proto(&body_decoded), body);
}

#[test]
fn summary_hashes_are_stable() {
    let header = sample_header();
    let again = sample_header();
    assert_eq!(header.body_hash, again.body_hash);
    assert_eq!(header.summary_hash, again.summary_hash);
    assert_eq!(
        hex::encode_upper(header.body_hash.as_bytes()),
        "A606345E926BC749571988564AA8D506FFB506D5B0E40A2BFD6B819594A74146"
    );
    assert_eq!(
        hex::encode_upper(header.summary_hash.as_bytes()),
        "2AC8B23360DFC3ED439281889CDBA3FA62CFBC1B23EC060A053C461C18CE866C"
    );
}

#[test]
fn flipped_body_byte_changes_body_and_summary_hash() {
    let header = sample_header();
    let mut flipped = BODY.to_vec();
    flipped[0] ^= 0xff;
    let other = header_for(&flipped, header.prev_summary_hash);
    assert_ne!(header.body_hash, other.body_hash);
    assert_ne!(header.summary_hash, other.summary_hash);
    assert_eq!(header.validators_hash, other.validators_hash);
    assert_eq!(header.consensus_hash, other.consensus_hash);
}

#[test]
fn load_rejects_hashes_that_do_not_recompute() {
    let header = sample_header();
    let mut proto = header.to_proto().unwrap();
    proto.validators_hash[0] ^= 0xff;
    assert_eq!(
        SummaryHeader::try_from_proto(&proto).unwrap_err(),
        Error::SummaryValidatorsHashMismatch
    );

    let mut proto = header.to_proto().unwrap();
    proto.next_validators_hash[0] ^= 0xff;
    assert_eq!(
        SummaryHeader::try_from_proto(&proto).unwrap_err(),
        Error::SummaryNextValidatorsHashMismatch
    );

    let mut proto = header.to_proto().unwrap();
    proto.consensus_hash[0] ^= 0xff;
    assert_eq!(
        SummaryHeader::try_from_proto(&proto).unwrap_err(),
        Error::SummaryConsensusHashMismatch
    );

    let mut proto = header.to_proto().unwrap();
    proto.body_hash[0] ^= 0xff;
    assert_eq!(
        SummaryHeader::try_from_proto(&proto).unwrap_err(),
        Error::SummaryHashMismatch
    );
}

#[test]
fn first_summary_parent_is_genesis_anchor() {
    let doc = GenesisDoc::from_json(GENESIS.as_bytes()).unwrap();
    let anchor = genesis_anchor_hash(&doc);
    assert_eq!(anchor, Hash::from_array(sum(doc.to_json().as_bytes())));
    let header = header_for(BODY, anchor);
    assert_eq!(header.prev_summary_hash, anchor);
    assert_eq!(
        hex::encode_upper(anchor.as_bytes()),
        "20950AA500EBC749D750A030AFD375D9E8B7CE063B598E8ADAFFABF2A04B9981"
    );
}
