//! Conflicting and non-conflicting cases from `TestDuplicateVoteEvidenceValidation`.
//!
//! The random-key `TestEvidenceList` is skipped. Light-client attack evidence is covered
//! by the evidence pool test.

use eld_tendermint_crypto::{PrivKey, sum};
use eld_tendermint_proto::types::SignedMsgType;
use eld_tendermint_types::{
    Block, BlockId, DuplicateVoteEvidence, Error, EvidenceList, PartSetHeader, Time, Txs,
    Validator, ValidatorSet, Vote,
};

const CHAIN_ID: &str = "test-chain";
const EMPTY_ROOT: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

fn complete_block_id(byte: u8) -> BlockId {
    BlockId {
        hash: vec![byte; 32],
        part_set_header: PartSetHeader {
            total: 1,
            hash: vec![byte.wrapping_add(1); 32],
        },
    }
}

fn signed_vote(priv_key: &PrivKey, block_id: BlockId) -> Vote {
    let pub_key = priv_key.public_key().expect("public key");
    let mut vote = Vote {
        vote_type: SignedMsgType::Precommit,
        height: 10,
        round: 1,
        block_id,
        timestamp: Time::from_unix_parts(1_546_300_800, 0),
        validator_address: pub_key.address().to_vec(),
        validator_index: 0,
        signature: vec![1],
    };
    vote.sign(priv_key, CHAIN_ID).expect("sign");
    vote
}

struct Fixture {
    evidence: DuplicateVoteEvidence,
    validator_set: ValidatorSet,
}

fn conflicting_pair() -> Fixture {
    let priv_key = PrivKey::generate();
    let pub_key = priv_key.public_key().expect("public key");
    let validator_set =
        ValidatorSet::new(vec![Validator::new(pub_key, 10)]).expect("validator set");
    let vote_a = signed_vote(&priv_key, complete_block_id(2));
    let vote_b = signed_vote(&priv_key, complete_block_id(1));
    let evidence = DuplicateVoteEvidence::new(
        vote_a,
        vote_b,
        Time::from_unix_parts(1_546_300_800, 0),
        &validator_set,
    )
    .expect("evidence");
    Fixture {
        evidence,
        validator_set,
    }
}

#[test]
fn conflicting_votes_validate_and_verify() {
    let fixture = conflicting_pair();
    fixture.evidence.validate_basic().expect("ordered");
    fixture
        .evidence
        .verify(CHAIN_ID, &fixture.validator_set)
        .expect("verify");
    assert!(fixture.evidence.vote_a.block_id.key() < fixture.evidence.vote_b.block_id.key());
}

#[test]
fn swapped_order_fails_validate_basic() {
    let mut fixture = conflicting_pair();
    std::mem::swap(&mut fixture.evidence.vote_a, &mut fixture.evidence.vote_b);
    assert_eq!(
        fixture.evidence.validate_basic(),
        Err(Error::DuplicateVoteOrder)
    );
}

#[test]
fn invalid_vote_type_fails_validate_basic() {
    let mut fixture = conflicting_pair();
    fixture.evidence.vote_a.vote_type = SignedMsgType::Proposal;
    assert_eq!(
        fixture.evidence.validate_basic(),
        Err(Error::InvalidVoteType)
    );
}

#[test]
fn same_block_id_fails_verify() {
    let priv_key = PrivKey::generate();
    let pub_key = priv_key.public_key().expect("public key");
    let validator_set =
        ValidatorSet::new(vec![Validator::new(pub_key, 10)]).expect("validator set");
    let block_id = complete_block_id(1);
    let evidence = DuplicateVoteEvidence::new(
        signed_vote(&priv_key, block_id.clone()),
        signed_vote(&priv_key, block_id),
        Time::GO_ZERO,
        &validator_set,
    )
    .expect("evidence");
    assert_eq!(
        evidence.verify(CHAIN_ID, &validator_set),
        Err(Error::EvidenceSameBlockId)
    );
}

#[test]
fn nil_block_id_against_complete_block_id_verifies() {
    let priv_key = PrivKey::generate();
    let pub_key = priv_key.public_key().expect("public key");
    let validator_set =
        ValidatorSet::new(vec![Validator::new(pub_key, 10)]).expect("validator set");
    let evidence = DuplicateVoteEvidence::new(
        signed_vote(&priv_key, complete_block_id(1)),
        signed_vote(&priv_key, BlockId::default()),
        Time::GO_ZERO,
        &validator_set,
    )
    .expect("evidence");
    assert!(evidence.vote_a.block_id.is_zero());
    evidence.validate_basic().expect("ordered");
    evidence.verify(CHAIN_ID, &validator_set).expect("verify");
}

#[test]
fn wrong_chain_id_fails_verify() {
    let fixture = conflicting_pair();
    assert!(
        fixture
            .evidence
            .verify("other-chain", &fixture.validator_set)
            .is_err()
    );
}

#[test]
fn mismatched_power_fails_verify() {
    let mut fixture = conflicting_pair();
    fixture.evidence.validator_power += 1;
    assert_eq!(
        fixture.evidence.verify(CHAIN_ID, &fixture.validator_set),
        Err(Error::EvidenceValidatorPowerMismatch)
    );

    fixture.evidence.validator_power -= 1;
    fixture.evidence.total_voting_power += 1;
    assert_eq!(
        fixture.evidence.verify(CHAIN_ID, &fixture.validator_set),
        Err(Error::EvidenceTotalPowerMismatch)
    );
}

#[test]
fn validator_missing_from_set_fails_verify() {
    let fixture = conflicting_pair();
    let other = PrivKey::generate();
    let other_set = ValidatorSet::new(vec![Validator::new(
        other.public_key().expect("public key"),
        10,
    )])
    .expect("validator set");
    assert_eq!(
        fixture.evidence.verify(CHAIN_ID, &other_set),
        Err(Error::ValidatorNotInSet)
    );
}

#[test]
fn hash_is_tmhash_of_bytes_and_proto_round_trip_keeps_it() {
    let fixture = conflicting_pair();
    assert_eq!(
        fixture.evidence.hash().as_bytes().as_slice(),
        sum(&fixture.evidence.bytes())
    );
    let decoded =
        DuplicateVoteEvidence::try_from_proto(&fixture.evidence.to_proto()).expect("round trip");
    assert_eq!(decoded.hash(), fixture.evidence.hash());
}

#[test]
fn block_evidence_hash_changes_when_the_list_is_not_empty() {
    let fixture = conflicting_pair();
    let empty = Block::make_block(3, Txs::new(vec![]), None, EvidenceList::default());
    assert_eq!(hex::encode(&empty.header.evidence_hash), EMPTY_ROOT);

    let list = EvidenceList::new(vec![fixture.evidence.into()]);
    let with = Block::make_block(3, Txs::new(vec![]), None, list.clone());
    assert_ne!(with.header.evidence_hash, empty.header.evidence_hash);
    assert_eq!(with.header.evidence_hash, list.hash().as_bytes().to_vec());
}
