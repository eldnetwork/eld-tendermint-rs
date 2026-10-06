# Pinned rust:1.86.0-bookworm, the toolchain this repo builds with.
# RocksDB is compiled in the node, so the build needs CMake and Clang.
FROM rust:1.86.0-bookworm AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        libclang-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN cargo build --release --locked -p eld-tendermint-node

# Bookworm slim, not Alpine. The binary links the C++ standard library from RocksDB.
FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        libgcc-s1 \
        libgomp1 \
        libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 tmuser \
    && useradd --uid 1000 --gid 1000 --create-home --shell /bin/bash tmuser \
    && mkdir -p /tendermint-rs/.tendermint/config /tendermint-rs/.tendermint/data \
    && chown -R tmuser:tmuser /tendermint-rs

COPY --from=build /src/target/release/eld-tendermint /usr/local/bin/eld-tendermint
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod 755 /usr/local/bin/eld-tendermint /usr/local/bin/docker-entrypoint.sh

ARG REVISION=unknown
LABEL org.opencontainers.image.source="https://github.com/eldnetwork/eld-tendermint-rs" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.revision="${REVISION}"

USER tmuser
WORKDIR /tendermint-rs

EXPOSE 26656 26657 26660
ENV TMHOME=/tendermint-rs/.tendermint

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -sf -X POST -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"health\"}" \
        http://127.0.0.1:26657/ || exit 1

ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["eld-tendermint", "start"]
