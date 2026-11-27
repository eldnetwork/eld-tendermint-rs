# eld-tendermint-content

Content push, fetch, announce, and capacity challenges on the Tendermint p2p switch.

`CONTENT_CHANNEL` is `0x50`. One message may be 1 MiB of blob bytes plus a small protobuf header. Anything longer is not queued. The node keeps a `ContentSendQueue`. `Reactor::from_shared` takes that queue, and the reactor polls it. `Reactor` calls `App` for each message a peer sends. A push, fetch, or challenge that the app accepts is answered on the next poll. A peer that sends bytes that are not one of these messages is stopped. An app refusal is logged and leaves the peer connected. A reply that does not fit on the queue is logged and dropped. A full queue is logged.

`ContentSendQueue::push`, `ContentSendQueue::request`, and `ContentSendQueue::challenge` start a message to one peer.

## Public API

`App` is the trait this crate calls for an inbound content message. `ContentSendQueue` is the queue of content messages this node will send. `Reactor` delivers inbound messages to `App` and sends that queue on `CONTENT_CHANNEL` (`0x50`). The node owns the queue and implements `App`. This crate does not open a socket and does not call ABCI itself.
