# Networking Layer Integration Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the new `dugite-network` crate (feature/networking-rewrite branch) with `dugite-node` so the full workspace builds, tests pass, and the node runs against preview testnet.

**Architecture:** The new networking crate preserves the core trait API (`BlockProvider`, `TxValidator`, `UtxoQueryProvider`, `ConnectionMetrics`) but replaces the old composite server types (`NodeServer`, `N2CServer`, `PeerManager`, `Governor`) with a new four-layer architecture (Bearer → Mux → Protocols → ConnectionManager). The integration proceeds in phases: first fix the trait adapter layer (serve.rs), then migrate query types, then replace the server/client infrastructure, and finally the sync pipeline.

**Tech Stack:** Rust, tokio, dugite-network (new API), dugite-node, dugite-cli

**Spec:** `docs/superpowers/specs/2026-03-24-networking-rewrite-design.md`

**Branch:** `feature/networking-rewrite` (17 networking tasks already complete, 154 tests)

---

## Dependency Map

The integration touches 4 files in dugite-node plus the workspace Cargo.toml. Changes must be done in order because each phase depends on the previous:

```
Phase 1: serve.rs (trait adapters — lowest risk, traits preserved)
Phase 2: query.rs (query types — move types into dugite-network or dugite-node)
Phase 3: mod.rs (server/peer infrastructure — replace NodeServer/PeerManager/Governor)
Phase 4: sync.rs (sync client — replace PipelinedPeerClient/NodeToNodeClient)
Phase 5: workspace cleanup (remove pallas-network, verify full build)
```

---

## File Structure

**Files modified in dugite-node:**
- `crates/dugite-node/src/node/serve.rs` — Update import paths (traits preserved)
- `crates/dugite-node/src/node/query.rs` — Move query snapshot types to dugite-network or inline
- `crates/dugite-node/src/node/mod.rs` — Replace server construction, peer management, connection handling
- `crates/dugite-node/src/node/sync.rs` — Replace PipelinedPeerClient with new ChainSync client

**Files modified in dugite-network:**
- `crates/dugite-network/src/lib.rs` — Add re-exports for backward compatibility
- `crates/dugite-network/src/protocol/chainsync/server.rs` — Re-export BlockAnnouncement

**Files modified in workspace:**
- `Cargo.toml` — Remove `pallas-network` from workspace dependencies
- `Cargo.lock` — Updated automatically

---

## Task 1: Fix serve.rs Import Paths

**Files:**
- Modify: `crates/dugite-node/src/node/serve.rs`
- Modify: `crates/dugite-network/src/lib.rs` (add UtxoQueryProvider re-export)

The traits `BlockProvider`, `TxValidator`, `ConnectionMetrics` are preserved in the new API with identical signatures. `UtxoQueryProvider` and `UtxoSnapshot` need to be importable from the new crate.

- [ ] **Step 1: Add UtxoQueryProvider re-export to network lib.rs**

The new crate already has `UtxoQueryProvider` and `UtxoSnapshot` at the top level. But serve.rs imports them from `dugite_network::query_handler`. Add a compatibility re-export module:

```rust
// In crates/dugite-network/src/lib.rs, add:
/// Backward-compatible re-exports for query handler types.
/// These were previously in dugite_network::query_handler.
pub mod query_handler {
    pub use super::{MultiAssetSnapshot, UtxoQueryProvider, UtxoSnapshot};
}
```

- [ ] **Step 2: Update serve.rs imports**

Change:
```rust
use dugite_network::query_handler::{UtxoQueryProvider, UtxoSnapshot};
use dugite_network::{BlockProvider, TipInfo, TxValidationError, TxValidator};
```

The `UtxoSnapshot` field names changed (`address_bytes` instead of `address`, `lovelace` instead of `value`, `multi_asset` instead of `multi_assets`). The new crate already has the correct field names matching the old API, so `utxo_to_snapshot()` at line 427 should work.

- [ ] **Step 3: Fix `utxo_to_snapshot` to use `query_handler::MultiAssetSnapshot`**

The old code references `dugite_network::query_handler::MultiAssetSnapshot` at line 431. This type alias is now `Vec<PolicyAssets>` which is `Vec<(Vec<u8>, Vec<(Vec<u8>, u64)>)>` — same underlying type.

- [ ] **Step 4: Verify serve.rs compiles**

Run: `cargo check -p dugite-node 2>&1 | grep -c "serve.rs"` — should be 0 (no errors from serve.rs)

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-network/src/lib.rs crates/dugite-node/src/node/serve.rs
git commit -m "fix(node): update serve.rs to use new networking API import paths"
```

---

## Task 2: Move Query Snapshot Types to dugite-node

**Files:**
- Modify: `crates/dugite-node/src/node/query.rs`
- Create: `crates/dugite-node/src/node/query_types.rs`

The old `dugite_network::query_handler` module contained ~20 snapshot types used exclusively by query.rs to build N2C responses. These types don't belong in the network crate — they're ledger-specific data shapes. Move them into the node crate.

- [ ] **Step 1: Create query_types.rs with all snapshot types**

Extract from the old `query_handler/types.rs` (available in git history at `HEAD~18:crates/dugite-network/src/query_handler/types.rs`):
- `SnapshotStakeData`
- `PoolParamsSnapshot`
- `RelaySnapshot`
- `ProtocolParamsSnapshot`
- `GovActionId`
- `NodeStateSnapshot`
- `EraSummary`
- `EraBound`

```bash
git show HEAD~18:crates/dugite-network/src/query_handler/types.rs > /tmp/old_types.rs
# Extract the needed types into query_types.rs
```

- [ ] **Step 2: Update query.rs imports to use local query_types**

Replace:
```rust
use dugite_network::query_handler::{PoolParamsSnapshot, RelaySnapshot, SnapshotStakeData};
```
With:
```rust
use super::query_types::{PoolParamsSnapshot, RelaySnapshot, SnapshotStakeData};
```

- [ ] **Step 3: Add query_types to node module tree**

In `crates/dugite-node/src/node/mod.rs`, add:
```rust
pub(crate) mod query_types;
```

- [ ] **Step 4: Verify query.rs compiles**

Run: `cargo check -p dugite-node 2>&1 | grep "query.rs" | head -5`

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/node/query_types.rs crates/dugite-node/src/node/query.rs crates/dugite-node/src/node/mod.rs
git commit -m "refactor(node): move query snapshot types from dugite-network to dugite-node"
```

---

## Task 3: Move Query Encoding Logic to dugite-node

**Files:**
- Modify: `crates/dugite-node/src/node/query.rs`
- Create: `crates/dugite-node/src/node/query_encoding.rs`

The old `query_handler/` module also contained CBOR encoding logic for query responses (protocol params, UTxO, stake distribution, governance, era history). This logic depends on ledger types and belongs in the node crate, not the network crate.

- [ ] **Step 1: Extract query encoding from git history**

The old files are:
- `query_handler/mod.rs` — QueryHandler struct, query dispatch, `encode_query_result`
- `query_handler/protocol.rs` — Era history, system start encoding
- `query_handler/stake.rs` — Stake distribution encoding
- `query_handler/governance.rs` — Governance query encoding
- `query_handler/utxo.rs` — UTxO query encoding

Extract the encoding functions into `query_encoding.rs` in the node crate.

- [ ] **Step 2: Implement the QueryHandler trait from the new network crate**

The new network crate defines `protocol::local_state_query::server::QueryHandler` with:
```rust
fn handle_block_query(&self, tag: u64, query_cbor: &[u8]) -> Result<Vec<u8>, String>;
fn handle_query_anytime(&self, query_cbor: &[u8]) -> Result<Vec<u8>, String>;
fn handle_query_hard_fork(&self, query_cbor: &[u8]) -> Result<Vec<u8>, String>;
fn validate_acquire(&self, target: &AcquireTarget) -> Result<(), AcquireFailure>;
```

Create a struct `NodeQueryHandler` in query_encoding.rs that implements this trait by dispatching tags 0-38 to the encoding functions.

- [ ] **Step 3: Wire NodeQueryHandler into mod.rs**

Replace the old `QueryHandler` field in the Node struct with `NodeQueryHandler` that implements the new trait.

- [ ] **Step 4: Verify query system compiles**

Run: `cargo check -p dugite-node 2>&1 | grep "query" | head -10`

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/node/query_encoding.rs crates/dugite-node/src/node/query.rs crates/dugite-node/src/node/mod.rs
git commit -m "refactor(node): implement QueryHandler trait for LocalStateQuery"
```

---

## Task 4: Replace Server Infrastructure in mod.rs

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs` (~3211 lines, heavy changes)

This is the largest and riskiest task. The old `NodeServer` composite type and `N2CServer` are replaced by the new `ConnectionManager`.

- [ ] **Step 1: Replace Node struct fields**

Remove fields:
- `_server: NodeServer`
- `n2c_server: Arc<N2CServer>`

Add:
- `connection_manager: Arc<ConnectionManager>`

Remove the old imports and add new ones from the new API.

- [ ] **Step 2: Replace N2N server initialization**

Old (mod.rs ~1375):
```rust
let n2n_server = N2NServer::with_config(config, block_provider, peer_manager, metrics);
```

New:
```rust
let connection_manager = ConnectionManager::new(ConnectionManagerConfig {
    max_inbound: 100,
    max_outbound: 20,
    network_magic: self.args.config.network_magic,
    peer_sharing: true,
    ..Default::default()
});
```

- [ ] **Step 3: Replace PeerManager initialization**

Old: `PeerManager::new(PeerManagerConfig { ... })`
New: `peer::PeerManager::new()` with topology peers added via `add_peer()`

- [ ] **Step 4: Replace Governor initialization**

Old: `Governor::new(GovernorConfig { targets: PeerTargets { ... } })`
New: `peer::Governor::new(peer::GovernorConfig { targets: peer::PeerTargets { ... } })`

- [ ] **Step 5: Replace N2C server initialization**

Old: `N2CServer::new(socket_path, query_handler, tx_validator, mempool, utxo_provider, metrics)`
New: Start a Unix listener task that accepts connections, runs handshake, and spawns LocalStateQuery/LocalTxSubmission/LocalChainSync/LocalTxMonitor servers on MuxChannels.

- [ ] **Step 6: Replace peer connection handling in the run loop**

The old code (~lines 1949-2465) uses `DuplexPeerConnection`, `NodeToNodeClient`, `TxSubmissionClient`, `request_peers_from()`. Replace with:
1. ConnectionManager accepts inbound / creates outbound
2. Mux splits into protocol channels
3. Spawn KeepAlive, ChainSync, BlockFetch, TxSubmission2, PeerSharing tasks on channels

- [ ] **Step 7: Replace BlockAnnouncement and RollbackAnnouncement**

Old: `dugite_network::BlockAnnouncement` / `RollbackAnnouncement`
New: `dugite_network::protocol::chainsync::server::BlockAnnouncement`

Add re-exports in lib.rs for backward compatibility.

- [ ] **Step 8: Verify mod.rs compiles (expect sync.rs errors)**

Run: `cargo check -p dugite-node 2>&1 | grep -v "sync.rs" | tail -20`

- [ ] **Step 9: Commit**

```bash
git add crates/dugite-node/src/node/mod.rs crates/dugite-network/src/lib.rs
git commit -m "feat(node): replace server infrastructure with ConnectionManager"
```

---

## Task 5: Replace Sync Client in sync.rs

**Files:**
- Modify: `crates/dugite-node/src/node/sync.rs` (~3243 lines)

Replace the old `PipelinedPeerClient` and `NodeToNodeClient` with the new `PipelinedChainSyncClient` from `dugite_network::protocol::chainsync::client`.

- [ ] **Step 1: Replace sync client types**

Old imports:
```rust
use dugite_network::{BlockFetchPool, ChainSyncEvent, EbbInfo, HeaderBatchResult, NodeToNodeClient, PipelinedPeerClient};
```

New imports:
```rust
use dugite_network::protocol::chainsync::client::{ChainSyncEvent, PipelinedChainSyncClient};
use dugite_network::protocol::blockfetch::client::BlockFetchClient;
```

- [ ] **Step 2: Replace the chain_sync_loop function**

The old `chain_sync_loop` used `PipelinedPeerClient` which wrapped pallas-network's ChainSync. The new client uses `PipelinedChainSyncClient::run()` with a callback.

Adapt the existing block processing pipeline to work with the new client's callback API:
```rust
client.run(&mut channel, |event| {
    match event {
        ChainSyncEvent::RollForward { header, tip_slot, .. } => { /* process block */ }
        ChainSyncEvent::RollBackward { point, .. } => { /* handle rollback */ }
        ChainSyncEvent::AtTip => { /* switch to tip mode */ }
    }
    Ok(())
}).await?;
```

- [ ] **Step 3: Replace block fetch integration**

Old: `BlockFetchPool` managed multiple fetchers
New: `BlockFetchClient::fetch_range()` per-channel, coordinated by `BlockFetchDecision`

- [ ] **Step 4: Replace EBB handling**

Old: `EbbInfo` type tracked EBB metadata
New: EBB detection can remain in sync.rs — it's not a network concern, it's a block parsing concern. Define a local `EbbInfo` struct.

- [ ] **Step 5: Replace announcement broadcasts**

Old: `BlockAnnouncement`, `RollbackAnnouncement` types from old API
New: `protocol::chainsync::server::BlockAnnouncement` (already defined in new crate)

- [ ] **Step 6: Verify sync.rs compiles**

Run: `cargo check -p dugite-node 2>&1 | tail -20`

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-node/src/node/sync.rs
git commit -m "feat(node): replace sync client with new PipelinedChainSyncClient"
```

---

## Task 6: Workspace Cleanup and Full Build

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/dugite-cli/Cargo.toml` (if it has dugite-network dep)

- [ ] **Step 1: Remove pallas-network from workspace dependencies**

In root `Cargo.toml`, remove:
```toml
pallas-network = { ... }
```

Confirm no other crate uses it (dugite-cli doesn't import from dugite-network).

- [ ] **Step 2: Full workspace build**

Run: `cargo build --all-targets 2>&1 | tail -20`
Expected: Clean build with zero warnings.

- [ ] **Step 3: Run all workspace tests**

Run: `cargo test --all 2>&1 | tail -30`
Expected: All tests pass.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 5: Run fmt check**

Run: `cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: remove pallas-network from workspace dependencies"
```

---

## Task 7: Testnet Verification

- [ ] **Step 1: Build release binary**

```bash
cargo build --release
```

- [ ] **Step 2: Run Mithril import**

```bash
./target/release/dugite-node mithril-import --network-magic 2 --database-path ./db-preview
```

- [ ] **Step 3: Start node and verify sync**

```bash
./target/release/dugite-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001
```

Verify:
- Node connects to peers
- ChainSync begins (blocks_received counter increases)
- N2C queries work (via dugite-cli)
- Prometheus metrics on port 12798

- [ ] **Step 4: Commit any fixes from testnet run**

---

## Risk Assessment

| Task | Risk | Mitigation |
|------|------|------------|
| Task 1 (serve.rs) | Low | Traits are preserved, mostly import path changes |
| Task 2 (query types) | Low | Move types, no logic changes |
| Task 3 (query encoding) | Medium | New QueryHandler trait impl, complex CBOR encoding |
| Task 4 (mod.rs server) | **High** | 3000+ line file, deep refactor of server lifecycle |
| Task 5 (sync.rs client) | **High** | 3000+ line file, core sync pipeline changes |
| Task 6 (workspace) | Low | Mechanical cleanup |
| Task 7 (testnet) | Medium | Integration bugs only found at runtime |

**Recommendation:** Tasks 1-3 can be done incrementally with partial compilation. Tasks 4-5 are the critical path and should be done together as they're interdependent (mod.rs creates the connections that sync.rs uses). Consider doing Task 4+5 as a single focused session.
