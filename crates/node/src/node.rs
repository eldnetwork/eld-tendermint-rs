//! Load one home, start the reactors, then serve RPC.

use std::env;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eld_tendermint_abci::SocketClient;
use eld_tendermint_blockchain::{
    Reactor as BlockchainReactor, channel_descriptors as blockchain_channels,
};
use eld_tendermint_config::{Config, load_home, resolve_home};
use eld_tendermint_consensus::{
    Node, NodeExtras, Reactor as ConsensusReactor, channel_descriptors as consensus_channels,
};
use eld_tendermint_crypto::marshal_pub_key;
use eld_tendermint_evidence::{
    Pool as EvidencePool, Reactor as EvidenceReactor, channel_descriptors as evidence_channels,
};
use eld_tendermint_mempool::{
    Mempool, Reactor as MempoolReactor, channel_descriptors as mempool_channels,
};
use eld_tendermint_p2p::{AddrBook, NodeKey, PexReactor, Switch, pex_channel_descriptors};
use eld_tendermint_privval::{FilePV, PrivValidator, RemoteSigner};
use eld_tendermint_proto::abci::RequestInfo;
use eld_tendermint_state::{
    CommitEvents, IndexTxs, StateStore, TM_CORE_SEMVER, TxIndex, load_or_init_chain,
};
use eld_tendermint_store::{BlockStore, RocksDb};
use eld_tendermint_types::{BLOCK_PROTOCOL, Level, log_line, upper_hex};

use crate::app::AbciApp;
use crate::error::{Error, fail};
use crate::rpc::{self, NodeStatus};
use crate::ws::{CommitPublisher, SubscriptionHub};

/// `eld-tendermint start [--home <dir>]` blocks on the RPC accept loop.
/// `eld-tendermint unsafe-reset-all` returns after wiping chain data.
///
/// # Errors
///
/// Returns the first startup or reset failure. The caller prints it and exits 1.
pub fn run() -> Result<(), Error> {
    match command_from_args()? {
        Command::Start(home) => {
            let process = boot(&home)?;
            let listener = TcpListener::bind(process.rpc_addr).map_err(fail)?;
            let bound = listener.local_addr().map_err(fail)?;
            println!("rpc: {bound}");
            let _ = io::stdout().flush();
            let rpc_addr = bound.to_string();
            log_line(
                Level::Info,
                "rpc",
                "RPC listener",
                &[("addr", rpc_addr.as_str())],
            );
            rpc::serve(listener, Arc::clone(&process.status))
        }
        Command::UnsafeResetAll {
            home,
            keep_addr_book,
        } => crate::reset::unsafe_reset_all(&home, keep_addr_book),
    }
}

enum Command {
    Start(PathBuf),
    UnsafeResetAll { home: PathBuf, keep_addr_book: bool },
}

struct NodeProcess {
    switch: Arc<Switch>,
    stop: Arc<AtomicBool>,
    poll: Option<JoinHandle<()>>,
    status: Arc<NodeStatus>,
    rpc_addr: SocketAddr,
}

impl Drop for NodeProcess {
    fn drop(&mut self) {
        log_line(Level::Info, "main", "Stopping Node", &[]);
        self.stop.store(false, Ordering::SeqCst);
        if let Some(handle) = self.poll.take() {
            let _ = handle.join();
        }
    }
}

fn boot(home: &Path) -> Result<NodeProcess, Error> {
    let config = load_home(home).map_err(fail)?;
    let block = BLOCK_PROTOCOL.to_string();
    log_line(
        Level::Info,
        "main",
        "Version info",
        &[
            ("tendermint_version", TM_CORE_SEMVER),
            ("block", block.as_str()),
            ("p2p", "8"),
        ],
    );
    let mut genesis = config.load_genesis().map_err(fail)?;
    let node_key = NodeKey::load(config.node_key_file()).map_err(fail)?;
    let node_id = node_key.id().map_err(fail)?;
    let key_file = config.node_key_file().display().to_string();
    log_line(
        Level::Info,
        "p2p",
        "P2P Node ID",
        &[("ID", node_id.as_str()), ("file", key_file.as_str())],
    );
    let pv: Box<dyn PrivValidator> = if config.base.priv_validator_laddr.is_empty() {
        Box::new(
            FilePV::load(
                config.priv_validator_key_file(),
                config.priv_validator_state_file(),
            )
            .map_err(fail)?,
        )
    } else {
        Box::new(
            RemoteSigner::dial(&config.base.priv_validator_laddr, genesis.chain_id.as_str())
                .map_err(fail)?,
        )
    };
    let block_store = open_block_store(&config)?;
    let state_store = Arc::new(open_state_store(&config)?);
    let app = connect_app(&config.base.proxy_app)?;
    let state = load_or_init_chain(&state_store, &mut genesis, |request| {
        app.init_chain(request)
    })
    .map_err(fail)?;
    if state.version.consensus.block != BLOCK_PROTOCOL {
        let software = BLOCK_PROTOCOL.to_string();
        let state_block = state.version.consensus.block.to_string();
        log_line(
            Level::Info,
            "main",
            "Software and state have different block protocols",
            &[
                ("software", software.as_str()),
                ("state", state_block.as_str()),
            ],
        );
    }
    let fast_sync = config.base.fast_sync && config.fastsync.version == "v0";
    let tx_index = open_tx_index(&config)?;
    let tx_for_reactors = tx_index
        .as_ref()
        .map(|index| Arc::clone(index) as Arc<dyn IndexTxs>);
    let subscriptions = Arc::new(SubscriptionHub::new());
    let events: Option<Arc<dyn CommitEvents>> =
        Some(Arc::new(CommitPublisher::new(Arc::clone(&subscriptions))));
    let blockchain = if fast_sync {
        Some(BlockchainReactor::new(
            Arc::clone(&block_store),
            Arc::clone(&state_store),
            state.clone(),
            app.clone(),
            tx_for_reactors.clone(),
            events.clone(),
        ))
    } else {
        None
    };
    let pub_key = pv.get_pub_key();
    let address_bytes = pub_key.address();
    let voting_power = genesis
        .validators
        .iter()
        .find(|validator| validator.address.as_slice() == address_bytes.as_slice())
        .map(|validator| validator.power)
        .unwrap_or(0);
    let validator_address = hex::encode(address_bytes).to_ascii_uppercase();
    let validator_message = if genesis
        .validators
        .iter()
        .any(|validator| validator.address.as_slice() == address_bytes.as_slice())
    {
        "This node is a validator"
    } else {
        "This node is not a validator"
    };
    log_line(
        Level::Info,
        "consensus",
        validator_message,
        &[("addr", validator_address.as_str())],
    );
    let pub_key_json = serde_json::from_str(&marshal_pub_key(&pub_key)).map_err(fail)?;

    let evidence_db = RocksDb::open(config.db_dir().join("evidence.db")).map_err(fail)?;
    let evidence_pool = EvidencePool::new(evidence_db, &state);
    let evidence_reactor = EvidenceReactor::new(evidence_pool.clone());
    let evidence_for_node = evidence_pool.for_proposal();
    let waiter = app.waiter();
    let rpc_app = app.clone();
    let mempool = Mempool::new(config.mempool.clone(), app.clone()).map_err(fail)?;
    let wal_path = config.consensus.wal_file();
    let node = Node::start_with_store_extras(
        config.consensus.clone(),
        pv,
        state,
        mempool,
        app,
        Arc::clone(&block_store),
        NodeExtras {
            wal_path: wal_path.is_file().then_some(wal_path),
            evidence: Some(evidence_for_node),
            tx_index: tx_for_reactors,
            events,
        },
    )
    .map_err(fail)?;
    let node_mempool = node.mempool();
    let mempool_reactor = MempoolReactor::from_shared(Arc::clone(&node_mempool));
    let consensus = ConsensusReactor::new(vec![node]);
    let pex = PexReactor::new(
        AddrBook::open(config.p2p.addr_book_file()),
        node_key.clone(),
    );

    let mempool_desc = mempool_channels(config.mempool.max_tx_bytes);
    let consensus_desc = consensus_channels();
    let pex_desc = pex_channel_descriptors();
    let blockchain_desc = if fast_sync {
        blockchain_channels()
    } else {
        Vec::new()
    };
    let evidence_desc = evidence_channels();
    let channels = hex::encode(
        mempool_desc
            .iter()
            .chain(consensus_desc.iter())
            .chain(pex_desc.iter())
            .chain(blockchain_desc.iter())
            .chain(evidence_desc.iter())
            .map(|desc| desc.id)
            .collect::<Vec<_>>(),
    )
    .to_ascii_uppercase();

    let switch = Arc::new(Switch::new());
    register(
        &switch,
        &consensus,
        &mempool_reactor,
        &pex,
        mempool_desc,
        consensus_desc,
        pex_desc,
    )?;
    if let Some(blockchain) = &blockchain {
        register_blockchain(&switch, blockchain, blockchain_desc)?;
    }
    register_evidence(&switch, &evidence_reactor, evidence_desc)?;

    let stop = Arc::new(AtomicBool::new(true));
    let consensus_status = consensus.clone();
    let poll = spawn_poll(
        Arc::clone(&stop),
        Arc::clone(&switch),
        consensus,
        mempool_reactor,
        pex,
        blockchain,
        evidence_reactor,
    );

    let status_switch = Arc::clone(&switch);
    let process = NodeProcess {
        switch,
        stop,
        poll: Some(poll),
        status: Arc::new(NodeStatus {
            id: node_id,
            network: genesis.chain_id.as_str().to_owned(),
            listen_addr: config.p2p.laddr.clone(),
            moniker: config.base.moniker.clone(),
            rpc_address: config.rpc.laddr.clone(),
            channels,
            address: validator_address,
            pub_key: pub_key_json,
            voting_power,
            block_store,
            mempool: node_mempool,
            waiter,
            commit_timeout: std_duration(config.rpc.timeout_broadcast_tx_commit),
            app: rpc_app,
            tx_index,
            subscriptions,
            genesis,
            state_store,
            switch: status_switch,
            consensus: consensus_status,
        }),
        rpc_addr: parse_tcp(&config.rpc.laddr)?,
    };
    process
        .switch
        .dial_persistent(&node_key, &config.p2p.persistent_peers)
        .map_err(fail)?;
    let p2p_bound = process
        .switch
        .listen(&node_key, &config.p2p.laddr)
        .map_err(fail)?;
    let p2p_addr = format!("tcp://{p2p_bound}");
    log_line(
        Level::Info,
        "p2p",
        "Listening",
        &[("addr", p2p_addr.as_str())],
    );
    Ok(process)
}

fn register(
    switch: &Arc<Switch>,
    consensus: &ConsensusReactor<AbciApp, AbciApp, RocksDb>,
    mempool: &MempoolReactor<AbciApp>,
    pex: &PexReactor,
    mempool_desc: Vec<eld_tendermint_p2p::ChannelDescriptor>,
    consensus_desc: Vec<eld_tendermint_p2p::ChannelDescriptor>,
    pex_desc: Vec<eld_tendermint_p2p::ChannelDescriptor>,
) -> Result<(), Error> {
    let mempool_cb = mempool.clone();
    let mempool_switch = Arc::downgrade(switch);
    switch
        .add_reactor("mempool", mempool_desc, move |peer_id, ch_id, bytes| {
            if mempool_cb.handle(peer_id, ch_id, &bytes) {
                return;
            }
            if let Some(switch) = mempool_switch.upgrade() {
                stop_bad_peer(&switch, peer_id);
            }
        })
        .map_err(fail)?;

    let consensus_cb = consensus.clone();
    let consensus_switch = Arc::downgrade(switch);
    switch
        .add_reactor("consensus", consensus_desc, move |peer_id, ch_id, bytes| {
            if consensus_cb.handle(peer_id, ch_id, &bytes) {
                return;
            }
            if let Some(switch) = consensus_switch.upgrade() {
                stop_bad_peer(&switch, peer_id);
            }
        })
        .map_err(fail)?;

    let pex_cb = pex.clone();
    let pex_switch = Arc::downgrade(switch);
    switch
        .add_reactor("pex", pex_desc, move |peer_id, _ch_id, bytes| {
            let Some(switch) = pex_switch.upgrade() else {
                return;
            };
            if !pex_cb.handle(&switch, peer_id, &bytes) {
                stop_bad_peer(&switch, peer_id);
            }
        })
        .map_err(fail)?;
    Ok(())
}

fn register_blockchain(
    switch: &Arc<Switch>,
    blockchain: &BlockchainReactor<AbciApp, RocksDb>,
    blockchain_desc: Vec<eld_tendermint_p2p::ChannelDescriptor>,
) -> Result<(), Error> {
    let blockchain_cb = blockchain.clone();
    let blockchain_switch = Arc::downgrade(switch);
    switch
        .add_reactor(
            "blockchain",
            blockchain_desc,
            move |peer_id, ch_id, bytes| {
                let Some(switch) = blockchain_switch.upgrade() else {
                    return;
                };
                if !blockchain_cb.handle(&switch, peer_id, ch_id, &bytes) {
                    stop_bad_peer(&switch, peer_id);
                }
            },
        )
        .map_err(fail)?;
    Ok(())
}

fn register_evidence(
    switch: &Arc<Switch>,
    evidence: &EvidenceReactor<RocksDb>,
    evidence_desc: Vec<eld_tendermint_p2p::ChannelDescriptor>,
) -> Result<(), Error> {
    let evidence_cb = evidence.clone();
    let evidence_switch = Arc::downgrade(switch);
    switch
        .add_reactor("evidence", evidence_desc, move |peer_id, ch_id, bytes| {
            let Some(switch) = evidence_switch.upgrade() else {
                return;
            };
            if !evidence_cb.handle(peer_id, ch_id, &bytes) {
                stop_bad_peer(&switch, peer_id);
            }
        })
        .map_err(fail)?;
    Ok(())
}

fn stop_bad_peer(switch: &Switch, peer_id: &str) {
    log_line(
        Level::Info,
        "p2p",
        "Peer stopped",
        &[("id", peer_id), ("err", "bad message")],
    );
    switch.stop_peer(peer_id);
}

fn spawn_poll(
    stop: Arc<AtomicBool>,
    switch: Arc<Switch>,
    consensus: ConsensusReactor<AbciApp, AbciApp, RocksDb>,
    mempool: MempoolReactor<AbciApp>,
    pex: PexReactor,
    blockchain: Option<BlockchainReactor<AbciApp, RocksDb>>,
    evidence: EvidenceReactor<RocksDb>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while stop.load(Ordering::SeqCst) {
            consensus.poll(&switch);
            mempool.poll(&switch);
            pex.poll(&switch);
            if let Some(blockchain) = &blockchain {
                blockchain.poll(&switch);
            }
            evidence.poll(&switch);
            thread::sleep(Duration::from_millis(5));
        }
    })
}

fn open_tx_index(config: &Config) -> Result<Option<Arc<TxIndex<RocksDb>>>, Error> {
    if config.tx_index.indexer == "null" {
        return Ok(None);
    }
    let db = RocksDb::open(config.db_dir().join("tx_index.db")).map_err(fail)?;
    Ok(Some(Arc::new(TxIndex::new(db))))
}

fn open_block_store(config: &Config) -> Result<Arc<BlockStore<RocksDb>>, Error> {
    let db_dir = config.db_dir();
    std::fs::create_dir_all(&db_dir).map_err(fail)?;
    let db = RocksDb::open(db_dir.join("blockstore")).map_err(fail)?;
    Ok(Arc::new(BlockStore::new(db)))
}

fn std_duration(duration: eld_tendermint_config::Duration) -> Duration {
    Duration::from_nanos(u64::try_from(duration.as_nanos()).unwrap_or(0))
}

fn open_state_store(config: &Config) -> Result<StateStore<RocksDb>, Error> {
    let db = RocksDb::open(config.db_dir().join("state")).map_err(fail)?;
    Ok(StateStore::new(db))
}

/// `dialRetryIntervalSeconds`. `mustConnect` is false, so a failed dial retries.
const DIAL_RETRY_INTERVAL: Duration = Duration::from_secs(3);

fn connect_app(proxy_app: &str) -> Result<AbciApp, Error> {
    log_line(
        Level::Info,
        "proxy",
        "Connecting to ABCI app",
        &[("proxy_app", proxy_app)],
    );
    let addr = proxy_app
        .strip_prefix("tcp://")
        .filter(|addr| !addr.is_empty())
        .ok_or_else(|| Error::new(format!("proxy_app is not tcp: {proxy_app}")))?;
    // `multiAppConn.OnStart` order: query, snapshot, mempool, consensus.
    // Each dial is `socketClient.OnStart` with `mustConnect == false`.
    let mut query = dial_abci(proxy_app, addr, "query");
    let snapshot = dial_abci(proxy_app, addr, "snapshot");
    let mempool = dial_abci(proxy_app, addr, "mempool");
    let consensus = dial_abci(proxy_app, addr, "consensus");
    let info = match query.info(RequestInfo {
        version: TM_CORE_SEMVER.to_owned(),
        block_version: BLOCK_PROTOCOL,
        p2p_version: 8,
    }) {
        Ok(info) => info,
        Err(err) => return Err(abci_failed(err)),
    };
    let app_version = info.app_version.to_string();
    let last_block_height = info.last_block_height.to_string();
    log_line(
        Level::Info,
        "consensus",
        "ABCI Handshake",
        &[
            ("app_version", app_version.as_str()),
            ("last_block_height", last_block_height.as_str()),
        ],
    );
    let app_hash = upper_hex(&info.last_block_app_hash);
    log_line(
        Level::Info,
        "consensus",
        "Completed ABCI Handshake - Tendermint and App are synced",
        &[
            ("appHeight", last_block_height.as_str()),
            ("appHash", app_hash.as_str()),
        ],
    );
    Ok(AbciApp::new(query, snapshot, mempool, consensus))
}

fn dial_abci(proxy_app: &str, addr: &str, connection: &'static str) -> SocketClient {
    loop {
        match SocketClient::connect(addr) {
            Ok(client) => return client,
            Err(err) => {
                let reason = err.to_string();
                let message = format!(
                    "abci.socketClient failed to connect to {proxy_app}. Retrying after 3s..."
                );
                log_line(
                    Level::Error,
                    "abci-client",
                    &message,
                    &[("connection", connection), ("err", reason.as_str())],
                );
                thread::sleep(DIAL_RETRY_INTERVAL);
            }
        }
    }
}

fn abci_failed(err: impl std::fmt::Display) -> Error {
    let failed = fail(err);
    let reason = failed.to_string();
    log_line(
        Level::Error,
        "proxy",
        "ABCI connection failed",
        &[("err", reason.as_str())],
    );
    failed
}

fn parse_tcp(laddr: &str) -> Result<SocketAddr, Error> {
    let addr = laddr
        .strip_prefix("tcp://")
        .ok_or_else(|| Error::new(format!("listen address is not tcp: {laddr}")))?;
    addr.parse()
        .map_err(|_| Error::new(format!("invalid listen address: {laddr}")))
}

fn command_from_args() -> Result<Command, Error> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(Error::new(
            "usage: eld-tendermint <start|unsafe-reset-all> [--home <dir>]",
        ));
    };
    let allow_keep_addr_book = match command.as_str() {
        "start" => false,
        "unsafe-reset-all" => true,
        _ => return Err(Error::new(format!("unknown command {command}"))),
    };
    let mut home = None;
    let mut keep_addr_book = false;
    while let Some(arg) = args.next() {
        if arg == "--home" {
            let Some(value) = args.next() else {
                return Err(Error::new("missing value for --home"));
            };
            home = Some(value);
        } else if let Some(value) = arg.strip_prefix("--home=") {
            if value.is_empty() {
                return Err(Error::new("missing value for --home"));
            }
            home = Some(value.to_owned());
        } else if allow_keep_addr_book && arg == "--keep-addr-book" {
            keep_addr_book = true;
        } else {
            return Err(Error::new(format!("unknown argument {arg}")));
        }
    }
    let home = resolve_home(home.as_deref()).map_err(fail)?;
    if allow_keep_addr_book {
        Ok(Command::UnsafeResetAll {
            home,
            keep_addr_book,
        })
    } else {
        Ok(Command::Start(home))
    }
}
