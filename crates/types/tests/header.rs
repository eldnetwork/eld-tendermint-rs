//! Golden vectors from `types/block_test.go` `TestHeaderHash` and header `ValidateBasic`.

use eld_tendermint_crypto::{address_hash, sum};
use eld_tendermint_types::{
    BLOCK_PROTOCOL, BlockId, ChainId, ConsensusVersion, Error, HASH_SIZE, Header, PartSetHeader,
    Time,
};

fn golden_header(validators_hash: Vec<u8>) -> Header {
    Header {
        version: ConsensusVersion { block: 1, app: 2 },
        chain_id: ChainId::new("chainId"),
        height: 3,
        time: Time::parse_rfc3339("2019-10-13T16:14:44Z").unwrap(),
        last_block_id: BlockId {
            hash: vec![0; HASH_SIZE],
            part_set_header: PartSetHeader {
                total: 6,
                hash: vec![0; HASH_SIZE],
            },
        },
        last_commit_hash: sum(b"last_commit_hash").to_vec(),
        data_hash: sum(b"data_hash").to_vec(),
        validators_hash,
        next_validators_hash: sum(b"next_validators_hash").to_vec(),
        consensus_hash: sum(b"consensus_hash").to_vec(),
        app_hash: sum(b"app_hash").to_vec(),
        last_results_hash: sum(b"last_results_hash").to_vec(),
        evidence_hash: sum(b"evidence_hash").to_vec(),
        proposer_address: address_hash(b"proposer_address").to_vec(),
    }
}

#[test]
fn header_hash_matches_go() {
    let header = golden_header(sum(b"validators_hash").to_vec());
    let hash = header.hash().unwrap();
    assert_eq!(
        hex::encode_upper(hash.as_bytes()),
        "F740121F553B5418C3EFBD343C2DBFE9E007BB67B0D020A0741374BAB65242A4"
    );
}

#[test]
fn empty_validators_hash_yields_no_header_hash() {
    assert!(golden_header(Vec::new()).hash().is_none());
}

#[test]
fn header_validate_basic_matches_go_rules() {
    let mut header = golden_header(sum(b"validators_hash").to_vec());
    header.version.block = BLOCK_PROTOCOL;
    assert!(header.validate_basic().is_ok());

    header.version.block = 1;
    assert!(matches!(
        header.validate_basic(),
        Err(Error::WrongBlockProtocol { got: 1 })
    ));
    header.version.block = BLOCK_PROTOCOL;

    header.height = 0;
    assert_eq!(header.validate_basic(), Err(Error::ZeroHeight));
    header.height = -1;
    assert_eq!(header.validate_basic(), Err(Error::NegativeHeight));
    header.height = 3;

    header.chain_id = ChainId::new("a".repeat(51));
    assert!(matches!(
        header.validate_basic(),
        Err(Error::ChainIdTooLong { len: 51 })
    ));
    header.chain_id = ChainId::new("a".repeat(50));
    assert!(header.validate_basic().is_ok());

    header.app_hash = vec![1, 2, 3];
    assert!(header.validate_basic().is_ok());
    header.data_hash = vec![1, 2, 3];
    assert!(matches!(
        header.validate_basic(),
        Err(Error::InvalidHashLength { len: 3 })
    ));
    header.data_hash = sum(b"data_hash").to_vec();

    header.proposer_address = vec![0; 19];
    assert!(matches!(
        header.validate_basic(),
        Err(Error::InvalidAddressLength { len: 19 })
    ));
}
