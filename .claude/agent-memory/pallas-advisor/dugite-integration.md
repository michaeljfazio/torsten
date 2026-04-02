---
name: dugite-pallas-integration
description: Exact pallas versions/features used per dugite crate, known workarounds, API quirks, and integration patterns
type: reference
---

# Dugite-Pallas Integration Details

Last reviewed: 2026-03-13, pallas v1.0.0-alpha.5

## Workspace-Level Dependency Declaration (Cargo.toml)

```toml
[workspace.dependencies]
pallas-network  = "1.0.0-alpha.5"
pallas-codec    = "1.0.0-alpha.5"
pallas-primitives = "1.0.0-alpha.5"
pallas-traverse = "1.0.0-alpha.5"
pallas-crypto   = { version = "1.0.0-alpha.5", features = ["kes"] }
pallas-addresses = "1.0.0-alpha.5"
```

## Per-Crate Usage

### dugite-crypto
```toml
pallas-crypto = { workspace = true }
```
Uses: `Sum6Kes`, `Sum6KesSig`, `KesSk` trait, `KesSig` trait, `KesPublicKey`, `PallasKesError`
File: `crates/dugite-crypto/src/kes.rs`

### dugite-network
```toml
pallas-network  = { workspace = true }
pallas-crypto   = { workspace = true }
pallas-traverse = { workspace = true }
```
Uses:
- `pallas_network::facades::{PeerClient, KeepAliveHandle, KeepAliveLoop}`
- `pallas_network::miniprotocols::{PROTOCOL_N2N_*, Point as PallasPoint}`
- `pallas_network::miniprotocols::chainsync::{HeaderContent, Message, Tip, NextResponse}`
- `pallas_network::miniprotocols::handshake`
- `pallas_network::miniprotocols::keepalive`
- `pallas_network::miniprotocols::blockfetch::Client` (stored in PipelinedPeerClient)
- `pallas_network::multiplexer::{Bearer, ChannelBuffer, Plexer, RunningPlexer, AgentChannel, MAX_SEGMENT_PAYLOAD_LENGTH}`
- `pallas_traverse::MultiEraHeader`

Files: `crates/dugite-network/src/pipelined.rs`, `client.rs`, `miniprotocols/peersharing.rs`, `miniprotocols/txsubmission.rs`

### dugite-serialization
```toml
pallas-primitives = { workspace = true }
pallas-traverse   = { workspace = true }
pallas-codec      = { workspace = true }
pallas-crypto     = { workspace = true }
pallas-addresses  = { workspace = true }
```
Uses: Heavy usage of pallas-primitives era types, pallas-traverse MultiEra* types, pallas-codec utils
File: `crates/dugite-serialization/src/multi_era.rs`

### dugite-node
```toml
pallas-addresses = { workspace = true }
pallas-crypto    = { workspace = true }
pallas-traverse  = { workspace = true }
```
Uses: Address parsing, crypto operations, block traversal

## Known API Workarounds

### 1. Pipelined ChainSync (Most Significant Divergence)

**Problem**: pallas-network ChainSync client enforces strict one-request-one-response ordering via its state machine. This gives one header per round-trip (~300ms RTT).

**Dugite's solution**: `PipelinedPeerClient` in `dugite-network/src/pipelined.rs` directly manipulates `ChannelBuffer` and `Plexer`/`RunningPlexer` internals. It manually encodes `chainsync::Message::RequestNext` using `ChannelBuffer::send_msg()` N times, then reads N `Message` responses.

**Impact**: 10-50x header throughput improvement. Default pipeline depth: 100 (configurable via `DUGITE_PIPELINE_DEPTH`).

**Risk**: Relies on pallas-network internals that could change. Specifically: the `ChannelBuffer` encoding of `Message` types and the plexer's `AgentChannel` structure.

### 2. 28-byte Hash Padding

**Problem**: Pallas uses `Hash<28>` for pool IDs, DRep keys, required signers, pool voter keys. Dugite needs these as 32-byte hashes internally.

**Rule**: Do NOT use `Hash<32>::from()` on a `Hash<28>` — this is a compile error or logic error. Must manually pad with 4 zero bytes.

**Pattern**:
```rust
let hash28: Hash<28> = ...; // from pallas
let mut hash32_bytes = [0u8; 32];
hash32_bytes[..28].copy_from_slice(hash28.as_ref());
let hash32 = Hash32::from(hash32_bytes);
```

### 3. KES Buffer Lifecycle

**Problem**: `Sum6Kes` implements `Drop` which zeroizes the key buffer via memsec. If bytes are needed after drop, they're gone.

**Pattern**: Copy bytes before any scope exit that would drop the key:
```rust
let key_bytes = {
    let kes = Sum6Kes::from_bytes(&mut buffer)?;
    let bytes = kes.as_bytes().to_vec(); // copy before drop
    bytes
};
```

### 4. Byron Epoch Length for Non-Mainnet

**Problem**: pallas has hardcoded mainnet constants in the `wellknown` module. For non-mainnet networks, epoch/slot lengths differ.

**Solution**: `PipelinedPeerClient` has a `byron_epoch_length` field. Setting to 0 uses pallas defaults (mainnet). Non-mainnet networks pass the correct value from genesis config.

Reference in code: `"0 = use pallas defaults (mainnet)"` comment in `pipelined.rs`.

### 5. DatumOption (was PseudoDatumOption)

After alpha.1 migration: always use `DatumOption`, never `PseudoDatumOption`. The rename was a breaking change from 0.x.

### 6. Option<T> (was Nullable<T>)

After alpha.1 migration: many fields that used `Nullable<T>` now use `Option<T>`. Use standard Rust `Option` patterns throughout.

## Cargo.lock Dual-Version Note

Dugite's Cargo.lock contains BOTH 0.33.0 and 1.0.0-alpha.5 versions of:
- pallas-addresses
- pallas-codec
- pallas-crypto
- pallas-primitives
- pallas-traverse

The 0.33.x versions are pulled in transitively (likely by cardano-lsm). This is not a problem — Cargo handles separate version trees. But it means there are ~10 pallas crates compiled, which increases build time.

**Action**: When upgrading cardano-lsm, check if it can align to 1.0.0-alpha.5 pallas versions to eliminate the 0.33.x duplicates.

## Pallas N2C Server Not Used

Pallas provides `NodeServer` facade for N2C server functionality, but dugite implements its own complete N2C server. This is necessary because:
1. Dugite's N2C server implements custom LocalStateQuery with 38 query types
2. N2C V17-V22 with bit-15 version encoding extends beyond pallas's V16 support
3. Dugite needs direct control over the response encoding (integer CBOR keys, HFC wrapper logic)

## Feature Flag Summary

| Crate | Features Enabled |
|-------|-----------------|
| pallas-crypto | `kes` |
| pallas-primitives | default (`json`) |
| pallas-addresses | default |
| pallas-codec | default |
| pallas-network | default |
| pallas-traverse | default |
