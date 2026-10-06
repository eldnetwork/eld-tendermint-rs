# eld-tendermint-crypto

Ed25519 key bytes, tmhash, Merkle roots, and Amino JSON for Tendermint 0.34.

Matches `crypto/crypto.go`, `crypto/tmhash`, `crypto/merkle`, and `crypto/ed25519` in the Go tree. Secp256k1 and the other curves are not implemented in this crate. The Merkle code builds an inclusion proof that a leaf sits in a root. Value-ops and key-path proofs are not implemented in this crate.

## Public API

- `PubKey` and `PrivKey` load raw key bytes, Amino-marshal them as `tendermint/PubKeyEd25519` and `tendermint/PrivKeyEd25519`, and `sign` or `verify` a message.
- `address_hash` returns the first 20 bytes of tmhash, which is the validator address. `sum` returns the full 32-byte tmhash. `sum_truncated` returns the first 20 bytes of tmhash.
- `hash_from_byte_slices` builds the RFC-6962 Merkle root of a list of leaves. `proofs_from_byte_slices` builds an inclusion `Proof` for each leaf passed in.
- `pub_key_to_proto` writes a `PubKey` into the generated `PublicKey` message. `pub_key_from_proto` reads a `PubKey` back out of a `PublicKey` message.
