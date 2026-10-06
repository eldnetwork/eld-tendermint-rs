# eld-tendermint-proto-compiler

The `prost-build` binary that regenerates `crates/proto/src/prost` from `proto/tendermint`.

This binary is not a Go package and not a library other crates link. `scripts/gen-proto.sh` runs this binary. `cargo test` does not run this binary. `scripts/gen-proto.sh` needs `protoc` on `PATH`.

```bash
scripts/gen-proto.sh
```

Do not hand-edit `crates/proto/src/prost`. Edit `proto/tendermint/**` only when the Go schema changes, then run `scripts/gen-proto.sh` again.
