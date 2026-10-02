# eld-tendermint-abci

Blocking Tendermint 0.17 ABCI socket client. Each call writes one request, then `Flush`, and reads both responses.

## Live node

`echo_and_info_against_node` is ignored, so CI does not need eld-node. It dials `ELD_ABCI_ADDR`, or `127.0.0.1:26658` when that variable is unset, calls Echo and Info, and checks that `ResponseInfo.version` is not empty.

```
ELD_ABCI_ADDR=127.0.0.1:26658 cargo test -p eld-tendermint-abci -- --ignored echo_and_info_against_node
```
