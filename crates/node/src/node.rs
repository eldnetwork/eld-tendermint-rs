//! Load one home, start the reactors, then serve RPC.

use std::env;
use std::io::{self, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use eld_tendermint_abci::SocketClient;
use eld_tendermint_blockchain::{
    Reactor as BlockchainReactor, channel_descriptors as blockchain_channels,
};
use eld_tendermint_config::{Config, load_home, resolve_home};
use eld_tendermint_consensus::{
    Node, Reactor as ConsensusReactor, channel_descriptors as consensus_channels,
};
use eld_tendermint_crypto::marshal_pub_key;
use eld_tendermint_mempool::{
    Mempool, Reactor as MempoolReactor, channel_descriptors as mempool_channels,
};
use eld_tendermint_p2p::{AddrBook, NodeKey, PexReactor, Switch, pex_channel_descriptors};
use eld_tendermint_privval::FilePV;
use eld_tendermint_proto::abci::RequestInfo;
use eld_tendermint_state::{StateStore, load_or_init_chain};
use eld_tendermint_store::{BlockStore, RocksDb};

use crate::app::AbciApp;
use crate::error::{Error, fail};
use crate::rpc::{self, NodeStatus};

/// `eld-tendermint start [--home <dir>]`. Blocks on the RPC accept loop.
///
/// # Errors
///
/// Returns the first startup failure. The caller prints it and exits 1.
pub fn run() -> Result<(), Error> {
    let home = home_from_args()?;
    let process = boot(&home)?;
    let listener = TcpListener::bind(process.rpc_addr).map_err(fail)?;
    let bound = listener.local_addr().map_err(fail)?;
    println!("rpc: {bound}");
    let _ = io::stdout().flush();
    rpc::serve(listener, Arc::clone(&process.status))
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
        self.stop.store(false, Ordering::SeqCst);
        if let Some(handle) = self.poll.take() {
            let _ = handle.join();
        }
    }
}

fn boot(home: &Path) -> Result<NodeProcess, Error> {
    let config = load_home(home).map_err(fail)?;
    let mut genesis = config.load_genesis().map_err(fail)?;
    let node_key = NodeKey::load(config.node_key_file()).map_err(fail)?;
    let pv = FilePV::load(
        config.priv_validator_key_file(),
        config.priv_validator_state_file(),
    )
    .map_err(fail)?;
    let block_store = open_block_store(&config)?;
    let state_store = Arc::new(open_state_store(&config)?);
    let app = connect_app(&config.base.proxy_app)?;
    let state = load_or_init_chain(&state_store, &mut genesis, |request| {
        app.init_chain(request)
    })
    .map_err(fail)?;
    let fast_sync = config.base.fast_sync && config.fastsync.version == "v0";
    let blockchain = if fast_sync {
        Some(BlockchainReactor::new(
            Arc::clone(&block_store),
            Arc::clone(&state_store),
            state.clone(),
            app.clone(),
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
    let pub_key_json = serde_json::from_str(&marshal_pub_key(&pub_key)).map_err(fail)?;
    let node_id = node_key.id().map_err(fail)?;

    let waiter = app.waiter();
    let rpc_app = app.clone();
    let mempool = Mempool::new(config.mempool.clone(), app.clone()).map_err(fail)?;
    let wal_path = config.consensus.wal_file();
    let node = if wal_path.is_file() {
        Node::start_with_wal_and_store(
            config.consensus.clone(),
            pv,
            state,
            mempool,
            app,
            Arc::clone(&block_store),
            &wal_path,
        )
    } else {
        Node::start_with_store(
            config.consensus.clone(),
            pv,
            state,
            mempool,
            app,
            Arc::clone(&block_store),
        )
    }
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
    let channels = hex::encode(
        mempool_desc
            .iter()
            .chain(consensus_desc.iter())
            .chain(pex_desc.iter())
            .chain(blockchain_desc.iter())
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

    let stop = Arc::new(AtomicBool::new(true));
    let poll = spawn_poll(
        Arc::clone(&stop),
        Arc::clone(&switch),
        consensus,
        mempool_reactor,
        pex,
        blockchain,
    );

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
        }),
        rpc_addr: parse_tcp(&config.rpc.laddr)?,
    };
    process
        .switch
        .dial_persistent(&node_key, &config.p2p.persistent_peers)
        .map_err(fail)?;
    process
        .switch
        .listen(&node_key, &config.p2p.laddr)
        .map_err(fail)?;
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
                switch.stop_peer(peer_id);
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
                switch.stop_peer(peer_id);
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
                switch.stop_peer(peer_id);
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
                    switch.stop_peer(peer_id);
                }
            },
        )
        .map_err(fail)?;
    Ok(())
}

fn spawn_poll(
    stop: Arc<AtomicBool>,
    switch: Arc<Switch>,
    consensus: ConsensusReactor<AbciApp, AbciApp, RocksDb>,
    mempool: MempoolReactor<AbciApp>,
    pex: PexReactor,
    blockchain: Option<BlockchainReactor<AbciApp, RocksDb>>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        while stop.load(Ordering::SeqCst) {
            consensus.poll(&switch);
            mempool.poll(&switch);
            pex.poll(&switch);
            if let Some(blockchain) = &blockchain {
                blockchain.poll(&switch);
            }
            thread::sleep(Duration::from_millis(5));
        }
    })
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

fn connect_app(proxy_app: &str) -> Result<AbciApp, Error> {
    let addr = proxy_app
        .strip_prefix("tcp://")
        .filter(|addr| !addr.is_empty())
        .ok_or_else(|| Error::new(format!("proxy_app is not tcp: {proxy_app}")))?;
    let mut client = SocketClient::connect(addr).map_err(fail)?;
    client
        .info(RequestInfo {
            version: "0.34.24".to_owned(),
            block_version: 11,
            p2p_version: 8,
        })
        .map_err(fail)?;
    Ok(AbciApp::new(Arc::new(Mutex::new(client))))
}

fn parse_tcp(laddr: &str) -> Result<SocketAddr, Error> {
    let addr = laddr
        .strip_prefix("tcp://")
        .ok_or_else(|| Error::new(format!("listen address is not tcp: {laddr}")))?;
    addr.parse()
        .map_err(|_| Error::new(format!("invalid listen address: {laddr}")))
}

fn home_from_args() -> Result<PathBuf, Error> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(Error::new("usage: eld-tendermint start [--home <dir>]"));
    };
    if command != "start" {
        return Err(Error::new(format!("unknown command {command}")));
    }
    let mut home = None;
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
        } else {
            return Err(Error::new(format!("unknown argument {arg}")));
        }
    }
    resolve_home(home.as_deref()).map_err(fail)
}
