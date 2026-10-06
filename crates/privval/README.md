# eld-tendermint-privval

Privval is the validator's signing key. The node uses it to sign votes and block proposals. This crate is Tendermint 0.34 `privval.FilePV`: it reads the key from `priv_validator_key.json` and the last signed height, round, and step from `priv_validator_state.json`. It can also dial a remote signer and ask that process to sign the vote or proposal.

Matches `privval.FilePV` and the remote-signer socket in the Go tree. The caller passes the path of each file. This crate does not read `config.toml`.

## Public API

`FilePV::load` reads the Amino-encoded validator key and the last height, round, and step that key signed. Signing the same vote or proposal again, at that same height, round, and step, returns the signature already stored. Signing different bytes at that same height, round, and step is rejected. The updated sign state is written to a temp file in the same directory, `fsync`ed, then renamed over `priv_validator_state.json`. `STEP_NONE`, `STEP_PROPOSE`, `STEP_PREVOTE`, and `STEP_PRECOMMIT` are the step numbers stored in `priv_validator_state.json`.

`RemoteSigner::dial` opens a TCP connection to the address in `priv_validator_laddr` and signs votes and proposals on that connection, instead of reading `priv_validator_key.json`. `read_delimited` and `write_delimited` read and write one length-prefixed privval message on the signer connection. If the dial is refused, `RemoteSigner::dial` returns an error, and `eld-tendermint start` exits before JSON-RPC listens.
