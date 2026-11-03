# eld-tendermint-content

Message types for content push, fetch, announce, and capacity challenges.

`CONTENT_CHANNEL` is `0x50`. One message may be 1 MiB of blob bytes plus a small protobuf header. Anything longer does not fit on the channel. The reactor that sends these messages is not registered yet.
