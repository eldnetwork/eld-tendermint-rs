//! Optional check against a running eld-node. Ignored so CI does not dial it.
//!
//! See the crate README.

use eld_tendermint_abci::SocketClient;
use eld_tendermint_proto::abci::RequestInfo;

#[test]
#[ignore = "requires eld-node; see crates/abci/README.md"]
fn echo_and_info_against_node() {
    let addr = std::env::var("ELD_ABCI_ADDR").unwrap_or_else(|_| "127.0.0.1:26658".to_owned());
    let mut client = SocketClient::connect(addr).expect("connect");
    client.echo("Hello").expect("echo");
    let info = client
        .info(RequestInfo {
            version: String::new(),
            block_version: 0,
            p2p_version: 0,
        })
        .expect("info");
    assert!(!info.version.is_empty());
}
