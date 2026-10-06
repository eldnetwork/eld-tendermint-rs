# eld-tendermint-abci

Blocking Tendermint ABCI 0.17 socket client. Each call writes one ABCI request, then a `Flush`, then reads the response to the request and the response to `Flush`.

Matches the Go socket client in `abci/client` for the length-prefixed `WriteMessage` frame. This crate does not accept ABCI connections and does not run consensus.

## Public API

`SocketClient` dials the ABCI app and calls `echo`, `info`, `check_tx`, `deliver_tx`, `commit`, `query`, `begin_block`, `end_block`, and `init_chain`. `write_message` and `read_message` write and read one zigzag length-prefixed ABCI frame.

## Live node

`echo_and_info_against_node` is marked ignored, so `cargo test` skips it and CI does not need an app process. The test dials `ELD_ABCI_ADDR`, or `127.0.0.1:26658` when `ELD_ABCI_ADDR` is unset, calls Echo and Info, and checks that `ResponseInfo.version` is not empty.

```bash
ELD_ABCI_ADDR=127.0.0.1:26658 cargo test -p eld-tendermint-abci -- --ignored echo_and_info_against_node
```
