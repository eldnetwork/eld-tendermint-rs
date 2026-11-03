# eld-tendermint-content

Message types for content push, fetch, announce, and capacity challenges.

`CONTENT_CHANNEL` is `0x50`. One message may be 1 MiB of blob bytes plus a small protobuf header. Anything longer is not queued. The node registers `Reactor` with the other reactors. A peer that sends bytes that are not one of these messages is stopped. The app socket that asks the reactor to send is not here yet.
