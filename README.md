# Torsten

A Cardano node implementation written in Rust, aiming for 100% compatibility with [cardano-node](https://github.com/IntersectMBO/cardano-node).

Built by [Sandstone Pool](https://www.sandstone.io/)

[![CI](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml/badge.svg)](https://github.com/michaeljfazio/torsten/actions/workflows/ci.yml)

## Architecture

Torsten is organized as a 10-crate Cargo workspace:

| Crate | Description |
|-------|-------------|
| `torsten-primitives` | Core types: hashes, blocks, transactions, addresses, values, protocol parameters (Byron–Conway) |
| `torsten-crypto` | Ed25519 keys, VRF, KES, text envelope format |
| `torsten-serialization` | CBOR encoding/decoding for Cardano wire format via pallas |
| `torsten-network` | Ouroboros mini-protocols (ChainSync, BlockFetch, TxSubmission, KeepAlive), N2N client/server, N2C server, multi-peer block fetch pool |
| `torsten-consensus` | Ouroboros Praos, chain selection, epoch transitions, slot leader checks |
| `torsten-ledger` | UTxO set, transaction validation, ledger state, certificate processing, native script evaluation, reward calculation |
| `torsten-mempool` | Thread-safe transaction mempool |
| `torsten-storage` | ChainDB (ImmutableDB via RocksDB with WriteBatch + VolatileDB in-memory) |
| `torsten-node` | Main binary, config, topology, pipelined chain sync loop |
| `torsten-cli` | cardano-cli compatible CLI |

### Crate Dependency Graph

```mermaid
graph TD
    NODE[torsten-node] --> NET[torsten-network]
    NODE --> CONS[torsten-consensus]
    NODE --> LEDGER[torsten-ledger]
    NODE --> STORE[torsten-storage]
    NODE --> POOL[torsten-mempool]
    CLI[torsten-cli] --> NET
    CLI --> PRIM[torsten-primitives]
    CLI --> CRYPTO[torsten-crypto]
    CLI --> SER[torsten-serialization]
    NET --> PRIM
    NET --> CRYPTO
    NET --> SER
    NET --> POOL
    CONS --> PRIM
    CONS --> CRYPTO
    LEDGER --> PRIM
    LEDGER --> CRYPTO
    LEDGER --> SER
    STORE --> PRIM
    STORE --> SER
    POOL --> PRIM
    SER --> PRIM
    CRYPTO --> PRIM
```

### Sync Pipeline

Torsten uses a pipelined multi-peer architecture for block synchronization:

```mermaid
flowchart LR
    subgraph Primary Peer
        CS[ChainSync<br/>Header Collection]
    end

    CS -->|headers| HQ[Header Queue]

    subgraph Block Fetch Pool
        BF1[Peer 1<br/>BlockFetch]
        BF2[Peer 2<br/>BlockFetch]
        BF3[Peer N<br/>BlockFetch]
    end

    HQ -->|range 1| BF1
    HQ -->|range 2| BF2
    HQ -->|range N| BF3

    BF1 -->|blocks| BP[Block Processor]
    BF2 -->|blocks| BP
    BF3 -->|blocks| BP

    BP --> CDB[(ChainDB)]
    BP --> LS[Ledger State]
```

### Storage Architecture

```mermaid
flowchart TD
    CDB[ChainDB] --> VOL[VolatileDB<br/>In-Memory BTreeMap<br/>Last k=2160 blocks]
    CDB --> IMM[ImmutableDB<br/>RocksDB with WriteBatch<br/>Permanent blocks]

    NEW[New Block] -->|add_block| VOL
    VOL -->|flush when > k blocks| IMM

    READ[Block Query] -->|1. check volatile| VOL
    READ -->|2. fallback to immutable| IMM

    ROLL[Rollback] -->|remove from volatile| VOL
```

### Relay Node Architecture

```mermaid
flowchart TB
    subgraph Inbound ["Inbound Connections"]
        IN1[Peer A] -->|N2N| MUX_IN[Multiplexer]
        IN2[Peer B] -->|N2N| MUX_IN
        IN3[Wallet] -->|N2C| MUX_N2C[N2C Server]
    end

    subgraph Outbound ["Outbound Connections"]
        MUX_OUT[Multiplexer] -->|ChainSync| PEER1[Bootstrap Peer]
        MUX_OUT -->|BlockFetch| PEER1
        MUX_OUT -->|TxSubmission| PEER1
    end

    subgraph Core ["Node Core"]
        PM[Peer Manager<br/>Cold→Warm→Hot]
        MP[Mempool<br/>Tx Validation]
        CDB[(ChainDB)]
        LS[Ledger State]
        CONS[Consensus<br/>Ouroboros Praos]
    end

    MUX_IN -->|ChainSync| CDB
    MUX_IN -->|BlockFetch| CDB
    MUX_IN -->|TxSubmission| MP
    MUX_N2C -->|LocalStateQuery| LS
    MUX_N2C -->|LocalTxSubmission| MP
    MUX_N2C -->|LocalTxMonitor| MP

    PEER1 -->|blocks| CDB
    CDB --> LS
    LS --> CONS
    PM -->|manage| MUX_OUT
    PM -->|manage| MUX_IN
```

### Ouroboros Protocol Stack

```mermaid
flowchart TB
    subgraph N2N ["Node-to-Node (TCP)"]
        HS[Handshake V14+]
        CSP[ChainSync<br/>Headers]
        BFP[BlockFetch<br/>Block Bodies]
        TX[TxSubmission2<br/>Transactions]
        KA[KeepAlive<br/>Liveness]
    end

    subgraph N2C ["Node-to-Client (Unix Socket)"]
        HSC[Handshake]
        LSQ[LocalStateQuery<br/>Ledger Queries]
        LTS[LocalTxSubmission<br/>Submit Transactions]
        LTM[LocalTxMonitor<br/>Mempool Queries]
    end

    MUX[Multiplexer] --> N2N
    MUX --> N2C
```

## Building

```bash
cargo build --release
```

## Running

```bash
# Run with default settings (mainnet)
cargo run --release --bin torsten-node -- run \
  --config config.json \
  --topology topology.json \
  --database-path ./db \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

### Cardano Preview Testnet

To sync against the Cardano preview testnet:

1. Create a `config-preview.json`:

```json
{
  "Network": "Testnet",
  "NetworkMagic": 2
}
```

2. Create a `topology-preview.json` with preview testnet relays:

```json
{
  "bootstrapPeers": [
    {
      "address": "preview-node.play.dev.cardano.org",
      "port": 3001
    }
  ],
  "localRoots": [{ "accessPoints": [], "advertise": false, "valency": 1 }],
  "publicRoots": [{ "accessPoints": [], "advertise": false }],
  "useLedgerAfterSlot": 102729600
}
```

> **Tip:** You can also download the official topology directly from the [Cardano Operations Book](https://book.world.dev.cardano.org/environments/preview/topology.json).

3. Run the node:

```bash
cargo run --release --bin torsten-node -- run \
  --config config-preview.json \
  --topology topology-preview.json \
  --database-path ./db-preview \
  --socket-path ./node-preview.sock \
  --host-addr 0.0.0.0 \
  --port 3001
```

The node will connect to the preview testnet, perform the N2N handshake, and begin syncing blocks. Progress is logged periodically showing slot, block number, UTxO count, epoch, and sync percentage.

#### Network Magic Values

| Network | Magic |
|---------|-------|
| Mainnet | `764824073` |
| Preview | `2` |
| Preprod | `1` |

## Testing

```bash
cargo test --all
```

## Development

Zero-warning policy enforced — all code must compile with `cargo clippy -- -D warnings` and pass `cargo fmt --check`.

## Feature Status

### Implemented

#### Core Infrastructure
- [x] 10-crate Cargo workspace architecture
- [x] CI pipeline (build, test, clippy, fmt)
- [x] Configuration file parsing (node config, genesis files)
- [x] Full P2P topology configuration (bootstrapPeers, localRoots, publicRoots, hotValency, warmValency, trustable, diffusionMode)
- [x] Byron, Shelley, Alonzo, Conway genesis file loading

#### Network (N2N — Node-to-Node)
- [x] Ouroboros N2N handshake (V14+, pallas 1.0)
- [x] ChainSync mini-protocol (header collection + block fetch)
- [x] BlockFetch mini-protocol (block retrieval, sub-batched with 100-block chunks)
- [x] TxSubmission2 mini-protocol (N2N server — message types)
- [x] KeepAlive mini-protocol (message types)
- [x] N2N server for inbound peer connections
- [x] Peer manager (cold/warm/hot lifecycle, failure backoff, exponential retry)
- [x] Bidirectional diffusion mode (InitiatorAndResponder)
- [x] DNS multi-resolution (all IPs per hostname for peer discovery)
- [x] Batched RocksDB WriteBatch for efficient volatile→immutable flush
- [x] PeerSharing mini-protocol (gossip-based peer discovery)
- [x] N2N ChainSync server (sequential block serving for inbound peers)
- [x] N2N TxSubmission2 mempool integration (serve tx IDs and bodies to peers)
- [x] Inbound connection rate limiting (per-IP token bucket DoS protection)
- [x] SIGHUP topology reload
- [x] Ledger-based peer discovery (SPO relay addresses from pool registrations, periodic sampling)

#### Network (N2C — Node-to-Client)
- [x] Unix domain socket server
- [x] N2C handshake
- [x] LocalStateQuery protocol
- [x] LocalTxSubmission protocol
- [x] LocalTxMonitor protocol
- [x] N2C client for CLI queries

#### Storage
- [x] ImmutableDB (RocksDB) for permanent blocks
- [x] VolatileDB (in-memory) for recent blocks with rollback
- [x] ChainDB combining immutable + volatile with k-deep flush
- [x] Batched RocksDB WriteBatch for efficient volatile→immutable flush
- [x] Tip recovery from persisted metadata on restart
- [x] Slot range queries (RocksDB iterator + BTreeMap range)
- [x] Ledger state snapshot save/restore

#### Consensus
- [x] Ouroboros Praos chain selection
- [x] Slot leader eligibility check (phi_f threshold)
- [x] VRF output validation (format checks)
- [x] KES period validation
- [x] Operational certificate Ed25519 signature verification
- [x] Epoch nonce computation

#### Ledger
- [x] UTxO set management (insert, remove, query)
- [x] Transaction validation (inputs, outputs, fees, TTL, value conservation)
- [x] Multi-asset (native token) tracking and validation
- [x] Certificate processing: stake registration/deregistration, delegation, pool registration/retirement
- [x] Native script evaluation (pubkey, all, any, n-of-k, timelocks)
- [x] Epoch transitions with mark/set/go stake snapshots
- [x] Reward calculation and distribution (monetary expansion, fee redistribution)
- [x] Treasury and reserves tracking
- [x] Collateral validation for Plutus transactions

#### Governance (Conway / CIP-1694)
- [x] DRep registration, update, deregistration
- [x] Vote delegation (DRep, always abstain, always no confidence)
- [x] Committee hot key authorization and resignation
- [x] Governance proposal submission
- [x] DRep/SPO/CC voting with per-action-type thresholds
- [x] Governance action ratification and enactment
- [x] Treasury withdrawals
- [x] Hard fork initiation ratification
- [x] No confidence motions

#### Queries (via N2C)
- [x] Chain tip query
- [x] Current epoch query
- [x] Current era query
- [x] Block number query
- [x] System start query
- [x] Protocol parameters query (live from node state)
- [x] UTxO query by address (pluggable UtxoQueryProvider)
- [x] Stake distribution query
- [x] Stake address info (delegation + rewards)
- [x] DRep state query
- [x] Committee state query
- [x] Governance state query

#### CLI (cardano-cli compatible)
- [x] Key generation (payment, stake, DRep)
- [x] Address building (payment, stake)
- [x] Transaction build, sign, view, txid
- [x] Transaction calculate-min-fee
- [x] Transaction submission
- [x] Stake address commands (registration, deregistration, delegation, vote delegation)
- [x] Pool retirement certificate
- [x] Governance and stake distribution queries

#### Serialization
- [x] Multi-era block decoding (Byron–Conway) via pallas
- [x] CBOR encoding for Cardano wire format
- [x] Byron address detection (CBOR 0x82/0x83 headers)

### Not Yet Implemented

#### Relay Node Compliance
- [x] TxSubmission2 flow control (ack/req counts, inflight tracking per peer)
- [ ] Concurrent ChainSync from multiple peers
- [ ] Adaptive peer selection (latency-based ranking, reputation)

#### Block Producer
- [ ] KES key management (generation, rotation, period tracking)
- [ ] VRF key management and slot leader check
- [ ] Block forging (create blocks from mempool transactions)
- [ ] Operational certificate generation and rotation
- [ ] Block announcement (propagate forged blocks to peers)
- [ ] Leader schedule calculation

#### Cryptographic Verification
- [ ] Full VRF proof verification (requires VRF library integration)
- [ ] Full KES signature verification (requires KES library integration)

#### Plutus Smart Contracts
- [x] Plutus V1/V2/V3 script evaluation via uplc CEK machine
- [x] Phase-2 validation (eval_phase_two_raw)
- [x] Plutus evaluation in LocalTxSubmission (Phase-1 + Phase-2 validation before mempool admission)
- [ ] Script context construction for CLI transaction building

#### Performance
- [ ] Mithril snapshot import for fast initial sync
- [ ] Parallel block validation
- [ ] Memory-mapped UTxO set for large chains

#### Full CLI Parity
- [ ] Node operational certificate commands
- [ ] KES key generation and rotation
- [x] Pool registration certificate (with relay and metadata support)
- [ ] Metadata submission
- [ ] Full query command set

#### Integration Testing
- [ ] Full testnet sync to tip
- [ ] Full mainnet sync to tip
- [ ] Interoperability testing with cardano-node

## License

MIT
