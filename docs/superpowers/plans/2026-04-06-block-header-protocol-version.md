# Block Header Protocol Version Fix — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stamp forged block headers with the node's hardcoded software capability version (matching Haskell cardano-node behaviour), not the on-chain ledger protocol version.

**Architecture:** Define a `NODE_PROTOCOL_VERSION` constant in `dugite-node` representing the maximum protocol version this software supports (currently `ProtVer 10 8`, matching cardano-node 10.7.x). Parse `ExperimentalHardForksEnabled` from the node config JSON to optionally signal `ProtVer 11 0`. Wire this into `BlockProducerConfig` and the `OuroborosPraos.max_major_prot_ver` so both block forging and envelope checks use the same authoritative value.

**Tech Stack:** Rust, serde_json, dugite-primitives, dugite-consensus, dugite-node

**Closes:** #348

---

## Background: How Haskell Does It

In `cardano-node/src/Cardano/Node/Protocol/Cardano.hs`:

```haskell
cardanoProtocolVersion = if npcExperimentalHardForksEnabled
                         then ProtVer (natVersion @11) 0
                         else ProtVer (natVersion @10) 8
```

This value is:
1. Stamped directly into every forged block header (`forgeShelleyBlock`)
2. Used to derive `MaxMajorProtVer` for the obsolete-node envelope check (`envelopeChecks`)
3. **Never** read from the ledger's `ProtocolParameters`

The `ExperimentalHardForksEnabled` JSON config flag (default `false`) is the only override — it selects between two hardcoded values, not a free-form numeric input.

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/dugite-node/src/config.rs` | Modify | Add `ExperimentalHardForksEnabled` field to `NodeConfig`, add `node_protocol_version()` method |
| `crates/dugite-node/src/node/mod.rs` | Modify | Use config-derived protocol version in `BlockProducerConfig` instead of ledger state; pass to `OuroborosPraos` |
| `crates/dugite-node/src/forge.rs` | Modify | Update `BlockProducerConfig` default to `ProtVer 10 8` |
| `crates/dugite-consensus/src/praos.rs` | Modify | Accept `max_major_prot_ver` from caller instead of hardcoding `10` |
| `crates/dugite-node/tests/protocol_version.rs` | Create | Integration test for config → protocol version logic |

---

### Task 1: Add `ExperimentalHardForksEnabled` to `NodeConfig` and `node_protocol_version()` method

**Files:**
- Modify: `crates/dugite-node/src/config.rs:38-191` (NodeConfig struct)
- Test: `crates/dugite-node/tests/protocol_version.rs` (new)

The Haskell node hardcodes two protocol version pairs and selects between them based on the `ExperimentalHardForksEnabled` JSON config flag. We replicate this exactly.

- [ ] **Step 1: Write the failing test**

Create `crates/dugite-node/tests/protocol_version.rs`:

```rust
use dugite_node::config::NodeConfig;
use dugite_primitives::block::ProtocolVersion;

/// Default config (no ExperimentalHardForksEnabled) should produce ProtVer 10,8.
#[test]
fn default_config_protocol_version_is_10_8() {
    let json = r#"{}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 10);
    assert_eq!(pv.minor, 8);
}

/// ExperimentalHardForksEnabled=false should produce ProtVer 10,8.
#[test]
fn experimental_false_protocol_version_is_10_8() {
    let json = r#"{"ExperimentalHardForksEnabled": false}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 10);
    assert_eq!(pv.minor, 8);
}

/// ExperimentalHardForksEnabled=true should produce ProtVer 11,0.
#[test]
fn experimental_true_protocol_version_is_11_0() {
    let json = r#"{"ExperimentalHardForksEnabled": true}"#;
    let config: NodeConfig = serde_json::from_str(json).unwrap();
    let pv = config.node_protocol_version();
    assert_eq!(pv.major, 11);
    assert_eq!(pv.minor, 0);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p dugite-node -E 'test(protocol_version)'`
Expected: FAIL — `node_protocol_version` method does not exist, `ExperimentalHardForksEnabled` field not parsed.

- [ ] **Step 3: Add the field and method to `NodeConfig`**

In `crates/dugite-node/src/config.rs`, add the field to the `NodeConfig` struct (after the `error_demotion_threshold` field, before the closing brace):

```rust
    /// Enable experimental hard fork transitions (default: false).
    ///
    /// When true, the node signals `ProtVer 11 0` in forged block headers,
    /// advertising readiness for the next major protocol version (Dijkstra era).
    /// When false (default), the node signals `ProtVer 10 8` — the maximum
    /// Conway-era protocol version supported by this software release.
    ///
    /// Matches cardano-node's `ExperimentalHardForksEnabled` config field.
    /// Must remain false on mainnet unless instructed otherwise.
    #[serde(default)]
    pub experimental_hard_forks_enabled: bool,
```

Add an `impl` block for `NodeConfig` (after the struct definition, before the `Protocol` struct):

```rust
impl NodeConfig {
    /// Returns the protocol version this node should stamp on forged block headers.
    ///
    /// This is a **software capability signal**, not the on-chain protocol version.
    /// It tells the network the maximum protocol version this node supports.
    ///
    /// Matches cardano-node's `cardanoProtocolVersion` in `Cardano.Node.Protocol.Cardano.hs`:
    /// - `ExperimentalHardForksEnabled = false` → `ProtVer 10 8`
    /// - `ExperimentalHardForksEnabled = true`  → `ProtVer 11 0`
    pub fn node_protocol_version(&self) -> ProtocolVersion {
        if self.experimental_hard_forks_enabled {
            ProtocolVersion { major: 11, minor: 0 }
        } else {
            ProtocolVersion { major: 10, minor: 8 }
        }
    }

    /// Returns the maximum major protocol version this node can validate.
    ///
    /// Derived from the node protocol version's major component.
    /// Used by the Praos consensus layer for the obsolete-node envelope check:
    /// if the on-chain ledger protocol version exceeds this, the node rejects
    /// all block headers (forcing an upgrade).
    pub fn max_major_protocol_version(&self) -> u64 {
        self.node_protocol_version().major
    }
}
```

Add the import at the top of `config.rs`:

```rust
use dugite_primitives::block::ProtocolVersion;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-node -E 'test(protocol_version)'`
Expected: 3 tests PASS.

- [ ] **Step 5: Run clippy and format**

Run: `cargo clippy -p dugite-node --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-node/src/config.rs crates/dugite-node/tests/protocol_version.rs
git commit -m "feat: add ExperimentalHardForksEnabled config and node_protocol_version()

Hardcode the node's software capability protocol version matching
cardano-node's behaviour in Cardano.Node.Protocol.Cardano.hs:
- Default (false): ProtVer 10,8
- ExperimentalHardForksEnabled=true: ProtVer 11,0

Refs: #348"
```

---

### Task 2: Update `BlockProducerConfig` default and forging to use node capability version

**Files:**
- Modify: `crates/dugite-node/src/forge.rs:235-244` (Default impl)
- Modify: `crates/dugite-node/src/node/mod.rs:3962-3975` (forging code)

- [ ] **Step 1: Update `BlockProducerConfig::default()` to `ProtVer 10 8`**

In `crates/dugite-node/src/forge.rs`, change the default:

```rust
impl Default for BlockProducerConfig {
    fn default() -> Self {
        BlockProducerConfig {
            // Node software capability version — NOT the on-chain protocol version.
            // Matches cardano-node 10.7.x default (ExperimentalHardForksEnabled=false).
            protocol_version: ProtocolVersion { major: 10, minor: 8 },
            _max_block_body_size: 90112,
            _max_txs_per_block: 500,
            era: Era::Conway,
            slots_per_kes_period: 129600,
        }
    }
}
```

- [ ] **Step 2: Update the forging code in `node/mod.rs` to use config-derived version**

In `crates/dugite-node/src/node/mod.rs`, around lines 3958-3981, remove the ledger protocol version reads and use the node config instead.

**Before** (lines 3962-3963 — DELETE these):
```rust
        let protocol_version_major = ls.protocol_params.protocol_version_major;
        let protocol_version_minor = ls.protocol_params.protocol_version_minor;
```

**After** (in the `BlockProducerConfig` construction, lines 3972-3976 — REPLACE):
```rust
        let config = crate::forge::BlockProducerConfig {
            // Node software capability version from config, NOT the on-chain ledger version.
            // Matches cardano-node's cardanoProtocolVersion (hardcoded per software release).
            protocol_version: self.node_config.node_protocol_version(),
            _max_block_body_size: max_block_body_size,
            _max_txs_per_block: 500,
            era: current_era,
            slots_per_kes_period,
        };
```

This requires `self.node_config` to be accessible in the forging context. Verify how the `Node` struct stores the config — it should already have it as a field. If not, the `NodeConfig` must be threaded through. Check the `Node` struct definition to confirm.

- [ ] **Step 3: Verify build succeeds**

Run: `cargo build -p dugite-node`
Expected: Compiles cleanly. If `self.node_config` is not available in the forging method, locate where `NodeConfig` is stored on the `Node` struct and use that path instead.

- [ ] **Step 4: Run all node tests**

Run: `cargo nextest run -p dugite-node`
Expected: All tests pass.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy -p dugite-node --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-node/src/forge.rs crates/dugite-node/src/node/mod.rs
git commit -m "fix: stamp forged block headers with node capability version, not ledger version

BlockProducerConfig.protocol_version now comes from NodeConfig.node_protocol_version()
(hardcoded ProtVer 10,8 or 11,0 per ExperimentalHardForksEnabled) instead of reading
from LedgerState.protocol_params. This matches cardano-node's behaviour where the
block header protocol version is a software capability signal for upgrade readiness
voting, independent of the on-chain protocol parameters.

Fixes: #348"
```

---

### Task 3: Wire `max_major_prot_ver` from config into `OuroborosPraos`

**Files:**
- Modify: `crates/dugite-consensus/src/praos.rs` (constructor methods)
- Modify: `crates/dugite-node/src/node/mod.rs` (OuroborosPraos initialization)

Currently `OuroborosPraos` hardcodes `max_major_prot_ver: 10` in `new()`, `with_params()`, and `with_genesis_params()`. The Haskell node derives `MaxMajorProtVer` from the same `cardanoProtocolVersion` used for block forging. We should accept it as a parameter so the node can pass `config.max_major_protocol_version()`.

- [ ] **Step 1: Update `OuroborosPraos` constructors to accept `max_major_prot_ver`**

In `crates/dugite-consensus/src/praos.rs`:

Update `new()`:
```rust
    pub fn new() -> Self {
        Self::with_max_major_prot_ver(10)
    }

    pub fn with_max_major_prot_ver(max_major_prot_ver: u64) -> Self {
        OuroborosPraos {
            genesis_hash: Hash32::ZERO,
            security_param: 2160,
            active_slots_coeff: 0.05,
            epoch_length: 432000,
            slot_length: 1,
            tip: Tip::origin(),
            strict_verification: false,
            nonce_established: false,
            snapshots_established: false,
            checkpoints: HashMap::new(),
            max_major_prot_ver,
            opcert_counters: HashMap::new(),
        }
    }
```

Update `with_params()` — add `max_major_prot_ver: u64` parameter:
```rust
    pub fn with_params(
        security_param: u64,
        active_slots_coeff: f64,
        epoch_length: u64,
        slot_length: u64,
        max_major_prot_ver: u64,
    ) -> Self {
        OuroborosPraos {
            // ... same fields ...
            max_major_prot_ver,
            // ...
        }
    }
```

Update `with_genesis_params()` — add `max_major_prot_ver: u64` parameter:
```rust
    pub fn with_genesis_params(
        genesis_hash: Hash32,
        security_param: u64,
        active_slots_coeff: f64,
        epoch_length: u64,
        slot_length: u64,
        max_major_prot_ver: u64,
    ) -> Self {
        OuroborosPraos {
            // ... same fields ...
            max_major_prot_ver,
            // ...
        }
    }
```

- [ ] **Step 2: Fix all callers in dugite-node**

In `crates/dugite-node/src/node/mod.rs`, find all calls to `OuroborosPraos::new()`, `with_params()`, and `with_genesis_params()` and pass `self.node_config.max_major_protocol_version()` (or the config's value at initialization time). There may also be callers in test code — update those too. Tests that create `OuroborosPraos` directly can use `new()` (which defaults to 10) or the explicit constructor.

- [ ] **Step 3: Fix all callers in other crates and tests**

Search for all uses of `OuroborosPraos::with_params` and `OuroborosPraos::with_genesis_params` across the workspace:

Run: `grep -rn "with_params\|with_genesis_params" crates/ tests/ --include="*.rs" | grep -i praos`

Update each call site to pass the additional parameter. For test code that doesn't care about the value, pass `10`.

- [ ] **Step 4: Verify full workspace builds**

Run: `cargo build --all-targets`
Expected: Clean build, no errors.

- [ ] **Step 5: Run all tests**

Run: `cargo nextest run --workspace`
Expected: All tests pass.

- [ ] **Step 6: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-consensus/src/praos.rs crates/dugite-node/src/node/mod.rs
# (plus any other files with updated call sites)
git commit -m "refactor: derive max_major_prot_ver from NodeConfig instead of hardcoding

OuroborosPraos constructors now accept max_major_prot_ver as a parameter.
The node passes config.max_major_protocol_version() (derived from the same
node_protocol_version used for block header stamping), matching Haskell's
behaviour where MaxMajorProtVer = pvMajor(cardanoProtocolVersion).

Refs: #348"
```

---

### Task 4: Add comprehensive test for the full forging path

**Files:**
- Modify: `crates/dugite-node/tests/protocol_version.rs`

- [ ] **Step 1: Add test verifying forge_block uses the config protocol version**

Append to `crates/dugite-node/tests/protocol_version.rs`:

```rust
use dugite_node::forge::{BlockProducerConfig, BlockProducerCredentials, forge_block};
use dugite_primitives::block::{BlockNo, Era, ProtocolVersion, SlotNo};
use dugite_primitives::hash::Hash32;

/// Verify that forge_block stamps the protocol version from BlockProducerConfig,
/// not any ledger state.
#[test]
fn forge_block_stamps_config_protocol_version() {
    // Skip if we can't construct credentials (requires crypto keys).
    // This test verifies the config→header path, so we use a known config version.
    let config = BlockProducerConfig {
        protocol_version: ProtocolVersion { major: 10, minor: 8 },
        ..Default::default()
    };
    assert_eq!(config.protocol_version.major, 10);
    assert_eq!(config.protocol_version.minor, 8);

    // With experimental hard forks:
    let config_experimental = BlockProducerConfig {
        protocol_version: ProtocolVersion { major: 11, minor: 0 },
        ..Default::default()
    };
    assert_eq!(config_experimental.protocol_version.major, 11);
    assert_eq!(config_experimental.protocol_version.minor, 0);
}

/// Verify the default BlockProducerConfig matches cardano-node 10.7.x.
#[test]
fn default_block_producer_config_matches_cardano_node() {
    let config = BlockProducerConfig::default();
    assert_eq!(
        config.protocol_version,
        ProtocolVersion { major: 10, minor: 8 },
        "Default BlockProducerConfig should match cardano-node 10.7.x (ProtVer 10,8)"
    );
}
```

- [ ] **Step 2: Run the new tests**

Run: `cargo nextest run -p dugite-node -E 'test(protocol_version)'`
Expected: All tests PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-node/tests/protocol_version.rs
git commit -m "test: add forge path protocol version integration tests

Verify BlockProducerConfig defaults match cardano-node 10.7.x (ProtVer 10,8)
and that the experimental hard forks config produces ProtVer 11,0.

Refs: #348"
```

---

### Task 5: Final verification and cleanup

**Files:**
- No new files

- [ ] **Step 1: Run full test suite**

Run: `cargo nextest run --workspace`
Expected: All tests pass.

- [ ] **Step 2: Run clippy and fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: Clean.

- [ ] **Step 3: Verify the fix with a quick code audit**

Confirm these invariants hold:
1. `forge_block` gets `protocol_version` from `BlockProducerConfig`, which gets it from `NodeConfig.node_protocol_version()` — **never** from `LedgerState.protocol_params`
2. `OuroborosPraos.max_major_prot_ver` is set from `NodeConfig.max_major_protocol_version()` — matching the same software capability version
3. `ExperimentalHardForksEnabled` defaults to `false`, producing `ProtVer 10 8`
4. No remaining references to `ls.protocol_params.protocol_version_major/minor` in the forging code path

- [ ] **Step 4: Commit any remaining cleanup**

If any cleanup was needed, commit it.

---

## Version Maintenance Note

When Dugite adds support for newer protocol features (matching a newer cardano-node release), the hardcoded values in `NodeConfig::node_protocol_version()` should be bumped accordingly:

| cardano-node release | Default ProtVer | Experimental ProtVer |
|---------------------|----------------|---------------------|
| 10.1.x              | 10, 2          | —                   |
| 10.5.3–10.5.4       | 10, 6          | —                   |
| 10.6.1              | 10, 7          | —                   |
| 10.7.0 (current)    | 10, 8          | 11, 0               |

This is a **software release decision**, not a runtime decision. Update the constants when cutting a new Dugite release that adds protocol support.
