//! `ValidateBasic` cases from `config/config_test.go`.

use eld_tendermint_config::{
    Config, ConsensusConfig, Duration, Error, FastSyncConfig, InstrumentationConfig, MempoolConfig,
    P2pConfig, RpcConfig, StateSyncConfig,
};
use eld_tendermint_types::Time;

fn assert_negative(result: Result<(), Error>, field: &str) {
    match result {
        Err(Error::Negative { field: got }) => assert_eq!(got, field),
        other => panic!("expected negative {field}, got {other:?}"),
    }
}

#[test]
fn default_and_test_configs_validate() {
    assert!(Config::default_config().validate_basic().is_ok());
    assert!(Config::test_config().validate_basic().is_ok());
    assert!(RpcConfig::test_config().validate_basic().is_ok());
    assert!(P2pConfig::test_config().validate_basic().is_ok());
    assert!(MempoolConfig::test_config().validate_basic().is_ok());
    assert!(StateSyncConfig::test_config().validate_basic().is_ok());
    assert!(FastSyncConfig::test_config().validate_basic().is_ok());
    assert!(ConsensusConfig::test_config().validate_basic().is_ok());
    assert!(
        InstrumentationConfig::test_config()
            .validate_basic()
            .is_ok()
    );
}

#[test]
fn config_validate_basic_rejects_negative_timeout() {
    let mut cfg = Config::default_config();
    cfg.consensus.timeout_propose = Duration::from_secs(-10);
    let err = cfg.validate_basic().unwrap_err();
    assert_eq!(
        err.to_string(),
        "error in [consensus] section: timeout_propose can't be negative"
    );
}

#[test]
fn base_log_format_must_be_plain_or_json() {
    let mut cfg = Config::test_config();
    assert!(cfg.validate_basic().is_ok());
    cfg.base.log_format = "invalid".to_owned();
    assert!(matches!(cfg.validate_basic(), Err(Error::UnknownLogFormat)));
}

#[test]
#[allow(clippy::type_complexity)]
fn rpc_rejects_negative_fields_and_small_buffers() {
    let fields: &[(&str, fn(&mut RpcConfig))] = &[
        ("grpc_max_open_connections", |cfg| {
            cfg.grpc_max_open_connections = -1;
        }),
        ("max_open_connections", |cfg| cfg.max_open_connections = -1),
        ("max_subscription_clients", |cfg| {
            cfg.max_subscription_clients = -1;
        }),
        ("max_subscriptions_per_client", |cfg| {
            cfg.max_subscriptions_per_client = -1;
        }),
        ("timeout_broadcast_tx_commit", |cfg| {
            cfg.timeout_broadcast_tx_commit = Duration::from_nanos(-1);
        }),
        ("max_body_bytes", |cfg| cfg.max_body_bytes = -1),
        ("max_header_bytes", |cfg| cfg.max_header_bytes = -1),
    ];
    for (field, set) in fields {
        let mut cfg = RpcConfig::test_config();
        set(&mut cfg);
        assert_negative(cfg.validate_basic(), field);
    }

    let mut cfg = RpcConfig::default_config();
    cfg.experimental_subscription_buffer_size = 99;
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::SubscriptionBufferTooSmall { got: 99 })
    ));
    cfg.experimental_subscription_buffer_size = 100;
    cfg.experimental_websocket_write_buffer_size = 99;
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::WebSocketBufferTooSmall { subscription: 100 })
    ));
}

#[test]
#[allow(clippy::type_complexity)]
fn p2p_rejects_negative_fields() {
    let fields: &[(&str, fn(&mut P2pConfig))] = &[
        ("max_num_inbound_peers", |cfg| {
            cfg.max_num_inbound_peers = -1
        }),
        ("max_num_outbound_peers", |cfg| {
            cfg.max_num_outbound_peers = -1;
        }),
        ("flush_throttle_timeout", |cfg| {
            cfg.flush_throttle_timeout = Duration::from_nanos(-1);
        }),
        ("max_packet_msg_payload_size", |cfg| {
            cfg.max_packet_msg_payload_size = -1;
        }),
        ("send_rate", |cfg| cfg.send_rate = -1),
        ("recv_rate", |cfg| cfg.recv_rate = -1),
        ("persistent_peers_max_dial_period", |cfg| {
            cfg.persistent_peers_max_dial_period = Duration::from_nanos(-1);
        }),
    ];
    for (field, set) in fields {
        let mut cfg = P2pConfig::test_config();
        set(&mut cfg);
        assert_negative(cfg.validate_basic(), field);
    }
}

#[test]
#[allow(clippy::type_complexity)]
fn mempool_rejects_negative_fields() {
    let fields: &[(&str, fn(&mut MempoolConfig))] = &[
        ("size", |cfg| cfg.size = -1),
        ("max_txs_bytes", |cfg| cfg.max_txs_bytes = -1),
        ("cache_size", |cfg| cfg.cache_size = -1),
        ("max_tx_bytes", |cfg| cfg.max_tx_bytes = -1),
    ];
    for (field, set) in fields {
        let mut cfg = MempoolConfig::test_config();
        set(&mut cfg);
        assert_negative(cfg.validate_basic(), field);
    }
}

#[test]
fn statesync_checks_only_when_enabled() {
    let mut cfg = StateSyncConfig::default_config();
    assert!(cfg.validate_basic().is_ok());
    cfg.enable = true;
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::RpcServersRequired)
    ));
    cfg.rpc_servers = vec!["http://one".to_owned()];
    assert!(matches!(cfg.validate_basic(), Err(Error::RpcServersTooFew)));
    cfg.rpc_servers = vec!["http://one".to_owned(), String::new()];
    assert!(matches!(cfg.validate_basic(), Err(Error::EmptyRpcServer)));
    cfg.rpc_servers = vec!["http://one".to_owned(), "http://two".to_owned()];
    cfg.discovery_time = Duration::from_secs(1);
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::DiscoveryTimeTooSmall)
    ));
    cfg.discovery_time = Duration::default();
    cfg.trust_period = Duration::default();
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::TrustPeriodRequired)
    ));
    cfg.trust_period = Duration::from_secs(1);
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::TrustHeightRequired)
    ));
    cfg.trust_height = 2;
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::TrustHashRequired)
    ));
    cfg.trust_hash = "zz".to_owned();
    assert!(matches!(cfg.validate_basic(), Err(Error::InvalidTrustHash)));
    cfg.trust_hash = "abcd".to_owned();
    cfg.chunk_request_timeout = Duration::from_secs(4);
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::ChunkRequestTimeoutTooSmall)
    ));
    cfg.chunk_request_timeout = Duration::from_secs(5);
    cfg.chunk_fetchers = 0;
    assert!(matches!(
        cfg.validate_basic(),
        Err(Error::ChunkFetchersRequired)
    ));
    cfg.chunk_fetchers = 4;
    assert!(cfg.validate_basic().is_ok());
    assert_eq!(
        cfg.trust_hash_bytes().unwrap(),
        hex::decode("abcd").unwrap()
    );
}

#[test]
fn fastsync_accepts_v0_v1_v2() {
    let mut cfg = FastSyncConfig::test_config();
    assert!(cfg.validate_basic().is_ok());
    cfg.version = "v1".to_owned();
    assert!(cfg.validate_basic().is_ok());
    cfg.version = "v2".to_owned();
    assert!(cfg.validate_basic().is_ok());
    cfg.version = "invalid".to_owned();
    let err = Config {
        fastsync: cfg,
        ..Config::default_config()
    }
    .validate_basic()
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "error in [fastsync] section: unknown fastsync version invalid"
    );
}

fn check_consensus(field: &str, expect_err: bool, modify: fn(&mut ConsensusConfig)) {
    let mut cfg = ConsensusConfig::default_config();
    modify(&mut cfg);
    let result = cfg.validate_basic();
    if expect_err {
        assert_negative(result, field);
    } else {
        assert!(result.is_ok(), "{field}");
    }
}

#[test]
fn consensus_timeout_sign_checks() {
    check_consensus("timeout_propose", false, |cfg| {
        cfg.timeout_propose = Duration::from_secs(1);
    });
    check_consensus("timeout_propose", true, |cfg| {
        cfg.timeout_propose = Duration::from_nanos(-1);
    });
    check_consensus("timeout_propose_delta", false, |cfg| {
        cfg.timeout_propose_delta = Duration::from_secs(1);
    });
    check_consensus("timeout_propose_delta", true, |cfg| {
        cfg.timeout_propose_delta = Duration::from_nanos(-1);
    });
    check_consensus("timeout_prevote", false, |cfg| {
        cfg.timeout_prevote = Duration::from_secs(1);
    });
    check_consensus("timeout_prevote", true, |cfg| {
        cfg.timeout_prevote = Duration::from_nanos(-1);
    });
    check_consensus("timeout_prevote_delta", false, |cfg| {
        cfg.timeout_prevote_delta = Duration::from_secs(1);
    });
    check_consensus("timeout_prevote_delta", true, |cfg| {
        cfg.timeout_prevote_delta = Duration::from_nanos(-1);
    });
    check_consensus("timeout_precommit", false, |cfg| {
        cfg.timeout_precommit = Duration::from_secs(1);
    });
    check_consensus("timeout_precommit", true, |cfg| {
        cfg.timeout_precommit = Duration::from_nanos(-1);
    });
    check_consensus("timeout_precommit_delta", false, |cfg| {
        cfg.timeout_precommit_delta = Duration::from_secs(1);
    });
    check_consensus("timeout_precommit_delta", true, |cfg| {
        cfg.timeout_precommit_delta = Duration::from_nanos(-1);
    });
    check_consensus("timeout_commit", false, |cfg| {
        cfg.timeout_commit = Duration::from_secs(1);
    });
    check_consensus("timeout_commit", true, |cfg| {
        cfg.timeout_commit = Duration::from_nanos(-1);
    });
    check_consensus("peer_gossip_sleep_duration", false, |cfg| {
        cfg.peer_gossip_sleep_duration = Duration::from_secs(1);
    });
    check_consensus("peer_gossip_sleep_duration", true, |cfg| {
        cfg.peer_gossip_sleep_duration = Duration::from_nanos(-1);
    });
    check_consensus("peer_query_maj23_sleep_duration", false, |cfg| {
        cfg.peer_query_maj23_sleep_duration = Duration::from_secs(1);
    });
    check_consensus("peer_query_maj23_sleep_duration", true, |cfg| {
        cfg.peer_query_maj23_sleep_duration = Duration::from_nanos(-1);
    });
    check_consensus("double_sign_check_height", true, |cfg| {
        cfg.double_sign_check_height = -1;
    });
    check_consensus("create_empty_blocks_interval", true, |cfg| {
        cfg.create_empty_blocks_interval = Duration::from_nanos(-1);
    });
}

#[test]
fn instrumentation_rejects_negative_connections() {
    let mut cfg = InstrumentationConfig::test_config();
    cfg.max_open_connections = -1;
    assert_negative(cfg.validate_basic(), "max_open_connections");
    let err = Config {
        instrumentation: cfg,
        ..Config::default_config()
    }
    .validate_basic()
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "error in [instrumentation] section: max_open_connections can't be negative"
    );
}

#[test]
fn consensus_timeout_helpers() {
    let cfg = ConsensusConfig::default_config();
    assert_eq!(cfg.propose(0), Duration::from_secs(3));
    assert_eq!(cfg.propose(2), Duration::from_secs(4));
    assert_eq!(cfg.prevote(1), Duration::from_millis(1500));
    assert_eq!(cfg.precommit(1), Duration::from_millis(1500));
    assert!(!cfg.wait_for_txs());
    let next = cfg.commit(Time::from_unix_parts(1_700_000_000, 0));
    assert_eq!(next.unix_seconds(), 1_700_000_001);
    assert_eq!(next.nanos(), 0);
}
