# ---- Build stage ----
FROM rust:1.88-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    libclang-dev clang pkg-config && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies by building them first
COPY Cargo.toml Cargo.lock ./
COPY crates/torsten-primitives/Cargo.toml crates/torsten-primitives/Cargo.toml
COPY crates/torsten-crypto/Cargo.toml crates/torsten-crypto/Cargo.toml
COPY crates/torsten-serialization/Cargo.toml crates/torsten-serialization/Cargo.toml
COPY crates/torsten-network/Cargo.toml crates/torsten-network/Cargo.toml
COPY crates/torsten-consensus/Cargo.toml crates/torsten-consensus/Cargo.toml
COPY crates/torsten-ledger/Cargo.toml crates/torsten-ledger/Cargo.toml
COPY crates/torsten-lsm/Cargo.toml crates/torsten-lsm/Cargo.toml
COPY crates/torsten-mempool/Cargo.toml crates/torsten-mempool/Cargo.toml
COPY crates/torsten-storage/Cargo.toml crates/torsten-storage/Cargo.toml
COPY crates/torsten-node/Cargo.toml crates/torsten-node/Cargo.toml
COPY crates/torsten-cli/Cargo.toml crates/torsten-cli/Cargo.toml
COPY crates/torsten-config/Cargo.toml crates/torsten-config/Cargo.toml
COPY crates/torsten-monitor/Cargo.toml crates/torsten-monitor/Cargo.toml
COPY crates/torsten-integration-tests/Cargo.toml crates/torsten-integration-tests/Cargo.toml

# Create dummy source files so cargo can resolve the workspace
RUN for dir in crates/torsten-*/; do \
      mkdir -p "$dir/src" && \
      echo "" > "$dir/src/lib.rs"; \
    done && \
    mkdir -p crates/torsten-node/src && echo "fn main(){}" > crates/torsten-node/src/main.rs && \
    mkdir -p crates/torsten-cli/src && echo "fn main(){}" > crates/torsten-cli/src/main.rs && \
    mkdir -p crates/torsten-config/src && echo "fn main(){}" > crates/torsten-config/src/main.rs && \
    mkdir -p crates/torsten-monitor/src && echo "fn main(){}" > crates/torsten-monitor/src/main.rs

RUN cargo build --release 2>/dev/null || true

# Copy real source and build
COPY . .

# Touch all source files so cargo rebuilds with real code
RUN find crates -name "*.rs" -exec touch {} +

RUN cargo build --release \
    --bin torsten-node \
    --bin torsten-cli \
    --bin torsten-config \
    --bin torsten-monitor

# ---- Prep stage: create dirs (distroless has no shell) ----
FROM debian:bookworm-slim AS prep

RUN mkdir -p /opt/torsten/db /opt/torsten/config /opt/torsten/ipc && \
    chown -R 65532:65532 /opt/torsten

COPY config/ /opt/torsten/config/

# ---- Runtime stage: distroless ----
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=prep /opt/torsten /opt/torsten
COPY --from=builder /build/target/release/torsten-node /usr/local/bin/torsten-node
COPY --from=builder /build/target/release/torsten-cli /usr/local/bin/torsten-cli
COPY --from=builder /build/target/release/torsten-config /usr/local/bin/torsten-config
COPY --from=builder /build/target/release/torsten-monitor /usr/local/bin/torsten-monitor

WORKDIR /opt/torsten

EXPOSE 3001 12798

VOLUME ["/opt/torsten/db", "/opt/torsten/ipc"]

ENTRYPOINT ["torsten-node"]
CMD ["run", \
     "--config", "/opt/torsten/config/preview-config.json", \
     "--topology", "/opt/torsten/config/preview-topology.json", \
     "--database-path", "/opt/torsten/db", \
     "--socket-path", "/opt/torsten/ipc/node.sock", \
     "--host-addr", "0.0.0.0", \
     "--port", "3001"]
