# ---- Build stage ----
FROM rust:1.93-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    libclang-dev clang pkg-config && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Cache dependencies by building them first
COPY Cargo.toml Cargo.lock ./
COPY crates/dugite-primitives/Cargo.toml crates/dugite-primitives/Cargo.toml
COPY crates/dugite-crypto/Cargo.toml crates/dugite-crypto/Cargo.toml
COPY crates/dugite-serialization/Cargo.toml crates/dugite-serialization/Cargo.toml
COPY crates/dugite-network/Cargo.toml crates/dugite-network/Cargo.toml
COPY crates/dugite-consensus/Cargo.toml crates/dugite-consensus/Cargo.toml
COPY crates/dugite-ledger/Cargo.toml crates/dugite-ledger/Cargo.toml
COPY crates/dugite-lsm/Cargo.toml crates/dugite-lsm/Cargo.toml
COPY crates/dugite-mempool/Cargo.toml crates/dugite-mempool/Cargo.toml
COPY crates/dugite-storage/Cargo.toml crates/dugite-storage/Cargo.toml
COPY crates/dugite-node/Cargo.toml crates/dugite-node/Cargo.toml
COPY crates/dugite-cli/Cargo.toml crates/dugite-cli/Cargo.toml
COPY crates/dugite-config/Cargo.toml crates/dugite-config/Cargo.toml
COPY crates/dugite-monitor/Cargo.toml crates/dugite-monitor/Cargo.toml
COPY crates/dugite-integration-tests/Cargo.toml crates/dugite-integration-tests/Cargo.toml

# Create dummy source files so cargo can resolve the workspace
RUN for dir in crates/dugite-*/; do \
      mkdir -p "$dir/src" && \
      echo "" > "$dir/src/lib.rs"; \
    done && \
    mkdir -p crates/dugite-node/src && echo "fn main(){}" > crates/dugite-node/src/main.rs && \
    mkdir -p crates/dugite-cli/src && echo "fn main(){}" > crates/dugite-cli/src/main.rs && \
    mkdir -p crates/dugite-config/src && echo "fn main(){}" > crates/dugite-config/src/main.rs && \
    mkdir -p crates/dugite-monitor/src && echo "fn main(){}" > crates/dugite-monitor/src/main.rs

RUN cargo build --release 2>/dev/null || true

# Copy real source and build
COPY . .

# Touch all source files so cargo rebuilds with real code
RUN find crates -name "*.rs" -exec touch {} +

RUN cargo build --release \
    --bin dugite-node \
    --bin dugite-cli \
    --bin dugite-config \
    --bin dugite-monitor

# ---- Prep stage: create dirs (distroless has no shell) ----
FROM debian:bookworm-slim AS prep

RUN mkdir -p /opt/dugite/db /opt/dugite/config /opt/dugite/ipc && \
    chown -R 65532:65532 /opt/dugite

COPY config/ /opt/dugite/config/

# ---- Runtime stage: distroless ----
FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=prep /opt/dugite /opt/dugite
COPY --from=builder /build/target/release/dugite-node /usr/local/bin/dugite-node
COPY --from=builder /build/target/release/dugite-cli /usr/local/bin/dugite-cli
COPY --from=builder /build/target/release/dugite-config /usr/local/bin/dugite-config
COPY --from=builder /build/target/release/dugite-monitor /usr/local/bin/dugite-monitor

WORKDIR /opt/dugite

EXPOSE 3001 12798

VOLUME ["/opt/dugite/db", "/opt/dugite/ipc"]

ENTRYPOINT ["dugite-node"]
CMD ["run", \
     "--config", "/opt/dugite/config/preview-config.json", \
     "--topology", "/opt/dugite/config/preview-topology.json", \
     "--database-path", "/opt/dugite/db", \
     "--socket-path", "/opt/dugite/ipc/node.sock", \
     "--host-addr", "0.0.0.0", \
     "--port", "3001"]
