# Security

## Private reports

Report a vulnerability in this repository in private:

https://github.com/eldnetwork/eld-tendermint-rs/security/advisories/new

Do not open a public issue for a vulnerability, a validator key, a node key, or any other secret.

Name the commit you ran, what an attacker can do, and how to reproduce it. Leave keys and the node home out of the report.

## What belongs in a private report

A flaw in this port: consensus, the peer handshake, signatures, the private validator, or an RPC method that exposes them.

A byte mismatch with the Go node that is not a vulnerability is a public compatibility issue. Use the compatibility template.

## Tendermint v0.34.24

This port is Tendermint Core v0.34.24. Report a flaw in this binary here. Name the commit you ran.

This repository does not run a bug bounty. The upstream Tendermint bounty does not cover this port.
