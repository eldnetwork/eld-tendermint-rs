//! Proposer selection from `types/validator_set_test.go`.
//!
//! Random keys, protobuf round-trips, commit checks, and validator updates are not here.

use eld_tendermint_types::{Error, Validator, ValidatorSet};

/// `TestProposerSelection1` expected addresses, 99 steps.
const SELECTION1: &str = "foo baz foo bar foo foo baz foo bar foo foo baz foo foo bar foo baz foo foo bar \
foo foo baz foo bar foo foo baz foo bar foo foo baz foo foo bar foo baz foo foo bar \
foo baz foo foo bar foo baz foo foo bar foo baz foo foo foo baz bar foo foo foo baz \
foo bar foo foo baz foo bar foo foo baz foo bar foo foo baz foo bar foo foo baz foo \
foo bar foo baz foo foo bar foo baz foo foo bar foo baz foo foo";

fn bare(address: impl Into<Vec<u8>>, power: i64) -> Validator {
    Validator {
        address: address.into(),
        pub_key: None,
        voting_power: power,
        proposer_priority: 0,
    }
}

fn addr(last: u8) -> Vec<u8> {
    let mut address = vec![0; 20];
    address[19] = last;
    address
}

fn proposer_address(set: &ValidatorSet) -> Vec<u8> {
    set.proposer().expect("proposer").address.clone()
}

fn sequence(set: &mut ValidatorSet, steps: usize) -> Vec<Vec<u8>> {
    let mut got = Vec::with_capacity(steps);
    for _ in 0..steps {
        got.push(proposer_address(set));
        set.increment_proposer_priority(1).expect("increment");
    }
    got
}

#[test]
fn proposer_selection1_matches_go_string() {
    let mut set = ValidatorSet::new(vec![
        bare(b"foo".to_vec(), 1000),
        bare(b"bar".to_vec(), 300),
        bare(b"baz".to_vec(), 330),
    ])
    .expect("set");
    let names: Vec<String> = sequence(&mut set, 99)
        .into_iter()
        .map(|address| String::from_utf8(address).expect("address"))
        .collect();
    let got = names.join(" ");
    assert_eq!(got, SELECTION1, "got\n{got}\nwant\n{SELECTION1}");
}

#[test]
fn proposer_selection2_matches_go() {
    let addr0 = addr(0);
    let addr1 = addr(1);
    let addr2 = addr(2);

    let mut set = ValidatorSet::new(vec![
        bare(addr0.clone(), 100),
        bare(addr1.clone(), 100),
        bare(addr2.clone(), 100),
    ])
    .expect("equal powers");
    let order = [addr0.clone(), addr1.clone(), addr2.clone()];
    for (step, expected) in sequence(&mut set, order.len() * 5).into_iter().enumerate() {
        assert_eq!(expected, order[step % order.len()], "step {step}");
    }

    let mut set = ValidatorSet::new(vec![
        bare(addr0.clone(), 100),
        bare(addr1.clone(), 100),
        bare(addr2.clone(), 400),
    ])
    .expect("power 400");
    assert_eq!(proposer_address(&set), addr2);
    set.increment_proposer_priority(1).expect("increment");
    assert_eq!(proposer_address(&set), addr0);

    let mut set = ValidatorSet::new(vec![
        bare(addr0.clone(), 100),
        bare(addr1.clone(), 100),
        bare(addr2.clone(), 401),
    ])
    .expect("power 401");
    assert_eq!(proposer_address(&set), addr2);
    set.increment_proposer_priority(1).expect("increment");
    assert_eq!(proposer_address(&set), addr2);
    set.increment_proposer_priority(1).expect("increment");
    assert_eq!(proposer_address(&set), addr0);

    let mut set = ValidatorSet::new(vec![
        bare(addr0.clone(), 4),
        bare(addr1.clone(), 5),
        bare(addr2.clone(), 3),
    ])
    .expect("powers 4 5 3");
    let mut counts = [0; 3];
    for address in sequence(&mut set, 120) {
        counts[usize::from(address[19])] += 1;
    }
    assert_eq!(counts, [40, 50, 30]);
}

#[test]
fn powers_one_and_two_repeat_high_low_high() {
    let low = addr(0);
    let high = addr(1);
    let mut set =
        ValidatorSet::new(vec![bare(low.clone(), 1), bare(high.clone(), 2)]).expect("set");
    let expected = [
        &high, &low, &high, &high, &low, &high, &high, &low, &high, &high, &low, &high, &high,
        &low, &high, &high, &low, &high, &high, &low,
    ];
    let got = sequence(&mut set, expected.len());
    assert_eq!(got.len(), 20);
    for (step, (got, want)) in got.iter().zip(expected).enumerate() {
        assert_eq!(got, want, "step {step}");
    }
}

#[test]
fn copy_increment_leaves_the_original_unchanged() {
    let set = ValidatorSet::new(vec![
        bare(b"foo".to_vec(), 1000),
        bare(b"bar".to_vec(), 300),
    ])
    .expect("set");
    let before: Vec<i64> = set
        .validators()
        .iter()
        .map(|validator| validator.proposer_priority)
        .collect();
    let proposer = proposer_address(&set);

    let mut copied = set.copy();
    copied
        .increment_proposer_priority(5)
        .expect("increment copy");

    let after: Vec<i64> = set
        .validators()
        .iter()
        .map(|validator| validator.proposer_priority)
        .collect();
    let copied_priorities: Vec<i64> = copied
        .validators()
        .iter()
        .map(|validator| validator.proposer_priority)
        .collect();
    assert_eq!(after, before);
    assert_eq!(proposer_address(&set), proposer);
    assert_ne!(copied_priorities, before);
}

#[test]
fn non_positive_times_are_rejected() {
    let mut set = ValidatorSet::new(vec![bare(b"foo".to_vec(), 1)]).expect("set");
    assert_eq!(
        set.increment_proposer_priority(0),
        Err(Error::NonPositiveTimes)
    );
    assert_eq!(
        set.increment_proposer_priority(-1),
        Err(Error::NonPositiveTimes)
    );
}
