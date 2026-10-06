# eld-tendermint-p2p

Tendermint 0.34 node identity, the secret-connection handshake, and the peer switch that carries channel messages.

Matches `p2p.NodeKey`, `p2p/conn` (the secret connection and the multiplexed connection), the switch, `addrbook.json`, and peer exchange (PEX) in the Go tree.

## Public API

`NodeKey` loads the Amino Ed25519 key in `node_key.json`. `SecretConnection` and `derive_secrets` run the handshake: X25519 ephemeral keys, a Merlin transcript, HKDF-SHA256, and ChaCha20-Poly1305 frames. `MConnection` multiplexes length-delimited packets on the secret-connection byte stream. `Switch` dials `persistent_peers`, accepts TCP connections, and delivers bytes to the handler registered for each channel id. `AddrBook` loads `addrbook.json`, or starts with an empty peer list when `addrbook.json` is missing. `PexReactor` exchanges `PexRequest` and `PexAddrs` on `PEX_CHANNEL` (`0x00`).

An unknown channel id, a payload larger than the channel's limit, or a bad frame disconnects the peer that sent the frame. Consensus and the mempool are not implemented in this crate. `eld-tendermint-consensus` and `eld-tendermint-mempool` register their channel ids on `Switch` and receive the bytes sent on the channel ids they registered.
