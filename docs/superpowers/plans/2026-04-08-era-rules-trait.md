# Era Rules Trait Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decompose the monolithic `LedgerState` into component sub-states and introduce an `EraRules` trait for per-era ledger rule dispatch, replacing ~25 scattered `protocol_version_major` guards.

**Architecture:** Split `LedgerState` (52 fields) into 5 component sub-states (`UtxoState`, `CertState`, `GovState`, `ConsensusState`, `EpochState`) with a `LedgerStateSnapshot` flat struct for backward-compatible serialization. Define an `EraRules` trait dispatched via an `EraRulesImpl` enum (not `dyn`) for zero-cost era-specific rule application. Each era (Byron, Shelley, Alonzo, Babbage, Conway) implements the trait by composing shared helpers from `eras/common.rs`.

**Tech Stack:** Rust, serde/bincode (snapshot serialization), dugite-primitives types

**Spec:** `docs/superpowers/specs/2026-04-08-era-rules-trait-design.md`

---

## File Map

### New files
| File | Responsibility |
|------|---------------|
| `crates/dugite-ledger/src/state/substates.rs` | `UtxoState`, `CertState`, `GovState`, `ConsensusState`, `EpochState` struct definitions |
| `crates/dugite-ledger/src/state/snapshot_format.rs` | `LedgerStateSnapshot` flat wire format + `From` conversions |
| `crates/dugite-ledger/src/eras/common.rs` | Shared helpers: Shelley base validation, UTxO mutation, cert processing, withdrawal draining, collateral consumption |
| `crates/dugite-ledger/src/eras/alonzo.rs` | `AlonzoRules` impl (Plutus, collateral, phase-2) |
| `crates/dugite-ledger/src/eras/babbage.rs` | `BabbageRules` impl (ref inputs, inline datums) |

### Modified files
| File | Change |
|------|--------|
| `crates/dugite-ledger/src/state/mod.rs` | Restructure `LedgerState` to use sub-states; add `mod substates; mod snapshot_format;` |
| `crates/dugite-ledger/src/state/snapshot.rs` | Use `LedgerStateSnapshot` for save/load instead of direct `LedgerState` bincode |
| `crates/dugite-ledger/src/state/apply.rs` | Rewrite `apply_block()` as thin orchestrator dispatching to `EraRulesImpl` |
| `crates/dugite-ledger/src/state/epoch.rs` | Extract epoch transition logic into era rule impls |
| `crates/dugite-ledger/src/state/certificates.rs` | Extract cert processing into era-specific + common functions |
| `crates/dugite-ledger/src/state/governance.rs` | Extract governance processing into Conway rule impl |
| `crates/dugite-ledger/src/state/rewards.rs` | Update field access paths to use sub-states |
| `crates/dugite-ledger/src/eras/mod.rs` | `EraRules` trait, `RuleContext`, `EraRulesImpl` enum, dispatch |
| `crates/dugite-ledger/src/eras/byron.rs` | Implement `EraRules` for `ByronRules` |
| `crates/dugite-ledger/src/eras/shelley.rs` | Replace empty marker struct with `ShelleyRules` impl |
| `crates/dugite-ledger/src/eras/conway.rs` | Replace empty marker struct with `ConwayRules` impl |
| `crates/dugite-ledger/src/lib.rs` | Update public re-exports |
| `crates/dugite-ledger/src/validation/mod.rs` | Update to use sub-state types |
| `crates/dugite-ledger/src/ledger_seq.rs` | Update field access paths |

---

## Task 1: Define Sub-State Structs

**Files:**
- Create: `crates/dugite-ledger/src/state/substates.rs`
- Modify: `crates/dugite-ledger/src/state/mod.rs`

This task defines the 5 sub-state structs. They are initially just grouping containers — no behavior moves yet. `LedgerState` keeps all its existing fields; the sub-states are defined alongside it.

- [ ] **Step 1: Create `substates.rs` with all 5 sub-state structs**

Create `crates/dugite-ledger/src/state/substates.rs`:

```rust
//! Component sub-states for LedgerState.
//!
//! These structs group related fields from the monolithic LedgerState into
//! independently borrowable components, enabling granular `&mut` access
//! for era-specific rule dispatch.
//!
//! Haskell equivalents:
//! - UtxoState  ≈ UTxOState
//! - CertState  ≈ CertState (DState + PState)
//! - GovState   ≈ ConwayGovState / GovState era
//! - ConsensusState ≈ ChainDepState + NewEpochState nonce fields
//! - EpochState ≈ EpochState + SnapShots + protocol parameters

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::network::NetworkId;
use dugite_primitives::time::{EpochNo, SlotNo};
use dugite_primitives::transaction::{ProtocolParamUpdate, TransactionInput, TransactionOutput};
use dugite_primitives::value::Lovelace;

use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;
use crate::utxo_diff::DiffSeq;

use super::{
    EpochSnapshots, GovernanceState, PendingRewardUpdate, PoolRegistration, Rational,
    StakeDistributionState,
};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::block::Tip;

/// UTxO state: the unspent transaction output set and per-epoch fee accumulator.
#[derive(Debug, Clone)]
pub struct UtxoSubState {
    pub utxo_set: UtxoSet,
    pub diff_seq: DiffSeq,
    pub epoch_fees: Lovelace,
    pub pending_donations: Lovelace,
}

/// Delegation and pool state: stake credentials, pool registrations, reward accounts.
#[derive(Debug, Clone)]
pub struct CertSubState {
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    pub future_pool_params: HashMap<Hash28, PoolRegistration>,
    pub pending_retirements: HashMap<Hash28, EpochNo>,
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
    pub stake_key_deposits: HashMap<Hash32, u64>,
    pub pool_deposits: HashMap<Hash28, u64>,
    pub total_stake_key_deposits: u64,
    pub pointer_map: HashMap<dugite_primitives::credentials::Pointer, Hash32>,
    pub stake_distribution: StakeDistributionState,
    pub script_stake_credentials: HashSet<Hash32>,
}

/// Governance state: proposals, votes, DReps, committee.
#[derive(Debug, Clone)]
pub struct GovSubState {
    pub governance: Arc<GovernanceState>,
}

/// Consensus-layer state: nonces, block production counters, opcert tracking.
#[derive(Debug, Clone)]
pub struct ConsensusSubState {
    pub evolving_nonce: Hash32,
    pub candidate_nonce: Hash32,
    pub epoch_nonce: Hash32,
    pub lab_nonce: Hash32,
    pub last_epoch_block_nonce: Hash32,
    pub rolling_nonce: Hash32,
    pub first_block_hash_of_epoch: Option<Hash32>,
    pub prev_epoch_first_block_hash: Option<Hash32>,
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    pub epoch_block_count: u64,
    pub opcert_counters: HashMap<Hash28, u64>,
}

/// Epoch-level state: snapshots, treasury/reserves, protocol parameters.
///
/// Protocol parameters live here because they change at epoch boundaries
/// (via governance enactment or pre-Conway PP update proposals). This allows
/// `process_epoch_transition` to mutate them via `&mut EpochSubState`.
#[derive(Debug, Clone)]
pub struct EpochSubState {
    pub snapshots: EpochSnapshots,
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub pending_reward_update: Option<PendingRewardUpdate>,
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    pub future_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    pub needs_stake_rebuild: bool,
    pub ptr_stake: HashMap<dugite_primitives::credentials::Pointer, u64>,
    pub ptr_stake_excluded: bool,
    pub protocol_params: ProtocolParameters,
    pub prev_protocol_params: ProtocolParameters,
    pub prev_protocol_version_major: u64,
    pub prev_d: f64,
}
```

- [ ] **Step 2: Add `mod substates;` to `state/mod.rs`**

In `crates/dugite-ledger/src/state/mod.rs`, add after the existing module declarations (line 7):

```rust
pub mod substates;
```

And add a public re-export after the existing use statements:

```rust
pub use substates::{
    CertSubState, ConsensusSubState, EpochSubState, GovSubState, UtxoSubState,
};
```

- [ ] **Step 3: Run tests to verify compilation**

Run: `cargo nextest run -p dugite-ledger --no-tests=pass 2>&1 | head -5`

Expected: Compiles successfully. All existing tests still pass (sub-states are defined but not yet used).

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p dugite-ledger --all-targets -- -D warnings 2>&1 | tail -20`

Expected: Clean (new structs may get dead_code warnings — add `#[allow(dead_code)]` temporarily on each struct if needed, to be removed when they're wired in).

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/substates.rs crates/dugite-ledger/src/state/mod.rs
git commit -m "refactor(ledger): define sub-state structs for LedgerState decomposition

Introduce UtxoSubState, CertSubState, GovSubState, ConsensusSubState, and
EpochSubState as grouping containers for the 52-field LedgerState. These
structs are defined but not yet wired in — LedgerState retains all fields.

Part of era-rules-trait refactor (#TBD)."
```

---

## Task 2: Define LedgerStateSnapshot Wire Format

**Files:**
- Create: `crates/dugite-ledger/src/state/snapshot_format.rs`
- Modify: `crates/dugite-ledger/src/state/mod.rs`

The flat `LedgerStateSnapshot` preserves the exact bincode field ordering of the current `LedgerState` for backward-compatible snapshot serialization. It will be used in a later task when we restructure `LedgerState`.

- [ ] **Step 1: Create `snapshot_format.rs` with the flat wire format struct**

Create `crates/dugite-ledger/src/state/snapshot_format.rs`. This struct must have fields in the **exact same order** as the current `LedgerState` (lines 86-348 of `state/mod.rs`), with the same serde attributes, so that existing snapshots deserialize correctly.

```rust
//! Flat wire format for LedgerState snapshots.
//!
//! `LedgerStateSnapshot` mirrors the original flat LedgerState field layout
//! for backward-compatible bincode serialization. The in-memory LedgerState
//! uses sub-state groupings for granular borrows; this struct is the stable
//! serialization boundary.
//!
//! IMPORTANT: Field order MUST match the historical LedgerState layout exactly.
//! Bincode is positional — reordering fields silently corrupts snapshots.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::ProtocolParamUpdate;
use dugite_primitives::value::Lovelace;

use crate::plutus::SlotConfig;
use crate::utxo::UtxoSet;

use super::{
    default_d_one, default_lovelace_zero, default_prev_proto_major,
    default_prev_protocol_params, default_update_quorum, EpochSnapshots,
    GovernanceState, PendingRewardUpdate, PoolRegistration,
    StakeDistributionState,
};
use dugite_primitives::block::Tip;
use dugite_primitives::protocol_params::ProtocolParameters;

/// Flat snapshot format matching the original LedgerState bincode layout.
///
/// Field order is load-bearing — do NOT reorder, insert, or remove fields
/// without bumping SNAPSHOT_VERSION and adding a migration path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LedgerStateSnapshot {
    // --- Fields in exact original order (state/mod.rs lines 86-348) ---
    pub utxo_set: UtxoSet,
    pub tip: Tip,
    pub era: Era,
    #[serde(skip, default)]
    pub pending_era_transition: Option<(Era, Era, EpochNo)>,
    pub epoch: EpochNo,
    pub epoch_length: u64,
    #[serde(default)]
    pub shelley_transition_epoch: u64,
    #[serde(default)]
    pub byron_epoch_length: u64,
    pub protocol_params: ProtocolParameters,
    #[serde(default = "default_prev_protocol_params")]
    pub prev_protocol_params: ProtocolParameters,
    #[serde(default = "default_d_one")]
    pub prev_d: f64,
    #[serde(default = "default_prev_proto_major")]
    pub prev_protocol_version_major: u64,
    pub stake_distribution: StakeDistributionState,
    pub treasury: Lovelace,
    #[serde(default = "default_lovelace_zero")]
    pub pending_donations: Lovelace,
    pub reserves: Lovelace,
    pub delegations: Arc<HashMap<Hash32, Hash28>>,
    pub pool_params: Arc<HashMap<Hash28, PoolRegistration>>,
    #[serde(default)]
    pub future_pool_params: HashMap<Hash28, PoolRegistration>,
    pub pending_retirements: HashMap<Hash28, EpochNo>,
    pub snapshots: EpochSnapshots,
    pub reward_accounts: Arc<HashMap<Hash32, Lovelace>>,
    #[serde(default)]
    pub pointer_map: HashMap<dugite_primitives::credentials::Pointer, Hash32>,
    #[serde(default)]
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    pub epoch_fees: Lovelace,
    pub epoch_blocks_by_pool: Arc<HashMap<Hash28, u64>>,
    pub epoch_block_count: u64,
    pub evolving_nonce: Hash32,
    pub candidate_nonce: Hash32,
    pub epoch_nonce: Hash32,
    pub lab_nonce: Hash32,
    pub last_epoch_block_nonce: Hash32,
    pub randomness_stabilisation_window: u64,
    #[serde(default)]
    pub stability_window_3kf: u64,
    pub genesis_hash: Hash32,
    #[serde(default)]
    pub rolling_nonce: Hash32,
    #[serde(default)]
    pub stability_window: u64,
    #[serde(default)]
    pub first_block_hash_of_epoch: Option<Hash32>,
    #[serde(default)]
    pub prev_epoch_first_block_hash: Option<Hash32>,
    pub pending_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    #[serde(default)]
    pub future_pp_updates: BTreeMap<EpochNo, Vec<(Hash32, ProtocolParamUpdate)>>,
    #[serde(default = "default_update_quorum")]
    pub update_quorum: u64,
    pub governance: Arc<GovernanceState>,
    pub slot_config: SlotConfig,
    #[serde(skip)]
    pub needs_stake_rebuild: bool,
    #[serde(default)]
    pub ptr_stake: HashMap<dugite_primitives::credentials::Pointer, u64>,
    #[serde(skip)]
    pub ptr_stake_excluded: bool,
    #[serde(default)]
    pub pending_reward_update: Option<PendingRewardUpdate>,
    #[serde(default)]
    pub total_stake_key_deposits: u64,
    #[serde(default)]
    pub script_stake_credentials: std::collections::HashSet<Hash32>,
    #[serde(skip)]
    pub diff_seq: crate::utxo_diff::DiffSeq,
    #[serde(skip)]
    pub node_network: Option<dugite_primitives::network::NetworkId>,
    #[serde(default)]
    pub opcert_counters: HashMap<Hash28, u64>,
    #[serde(default)]
    pub stake_key_deposits: HashMap<Hash32, u64>,
    #[serde(default)]
    pub pool_deposits: HashMap<Hash28, u64>,
}
```

- [ ] **Step 2: Add `mod snapshot_format;` to `state/mod.rs`**

In `crates/dugite-ledger/src/state/mod.rs`, add after `mod substates;`:

```rust
pub mod snapshot_format;
```

And add re-export:

```rust
pub use snapshot_format::LedgerStateSnapshot;
```

- [ ] **Step 3: Run tests to verify compilation**

Run: `cargo nextest run -p dugite-ledger --no-tests=pass 2>&1 | head -5`

Expected: Compiles. No test changes needed yet.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/snapshot_format.rs crates/dugite-ledger/src/state/mod.rs
git commit -m "refactor(ledger): add LedgerStateSnapshot flat wire format

Defines the stable bincode serialization format matching the original
LedgerState field layout. This decouples the in-memory sub-state
organization from the on-disk snapshot format."
```

---

## Task 3: Add Conversion Between LedgerState and LedgerStateSnapshot

**Files:**
- Modify: `crates/dugite-ledger/src/state/snapshot_format.rs`

Add `From` conversions between `LedgerState` and `LedgerStateSnapshot` so that snapshot save/load can roundtrip through the flat format. This task also adds a roundtrip test to prove the conversion is lossless.

- [ ] **Step 1: Write a roundtrip test**

Append to `crates/dugite-ledger/src/state/snapshot_format.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::LedgerState;

    #[test]
    fn test_ledger_state_snapshot_roundtrip() {
        // Create a LedgerState with non-default values to catch field mismatches
        let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
        state.epoch = EpochNo(42);
        state.treasury = Lovelace(1_000_000);
        state.reserves = Lovelace(999_000_000);

        // Convert to snapshot format
        let snapshot = LedgerStateSnapshot::from(&state);

        // Convert back
        let restored = LedgerState::from(snapshot);

        // Verify key fields survive the roundtrip
        assert_eq!(restored.epoch, state.epoch);
        assert_eq!(restored.treasury, state.treasury);
        assert_eq!(restored.reserves, state.reserves);
        assert_eq!(restored.era, state.era);
        assert_eq!(restored.protocol_params.protocol_version_major,
                   state.protocol_params.protocol_version_major);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_ledger_state_snapshot_roundtrip)'`

Expected: FAIL — `From` impls don't exist yet.

- [ ] **Step 3: Implement `From<&LedgerState> for LedgerStateSnapshot`**

Add to `crates/dugite-ledger/src/state/snapshot_format.rs` (before the `#[cfg(test)]` module):

```rust
impl From<&super::LedgerState> for LedgerStateSnapshot {
    fn from(s: &super::LedgerState) -> Self {
        Self {
            utxo_set: s.utxo_set.clone(),
            tip: s.tip.clone(),
            era: s.era,
            pending_era_transition: s.pending_era_transition,
            epoch: s.epoch,
            epoch_length: s.epoch_length,
            shelley_transition_epoch: s.shelley_transition_epoch,
            byron_epoch_length: s.byron_epoch_length,
            protocol_params: s.protocol_params.clone(),
            prev_protocol_params: s.prev_protocol_params.clone(),
            prev_d: s.prev_d,
            prev_protocol_version_major: s.prev_protocol_version_major,
            stake_distribution: s.stake_distribution.clone(),
            treasury: s.treasury,
            pending_donations: s.pending_donations,
            reserves: s.reserves,
            delegations: s.delegations.clone(),
            pool_params: s.pool_params.clone(),
            future_pool_params: s.future_pool_params.clone(),
            pending_retirements: s.pending_retirements.clone(),
            snapshots: s.snapshots.clone(),
            reward_accounts: s.reward_accounts.clone(),
            pointer_map: s.pointer_map.clone(),
            genesis_delegates: s.genesis_delegates.clone(),
            epoch_fees: s.epoch_fees,
            epoch_blocks_by_pool: s.epoch_blocks_by_pool.clone(),
            epoch_block_count: s.epoch_block_count,
            evolving_nonce: s.evolving_nonce,
            candidate_nonce: s.candidate_nonce,
            epoch_nonce: s.epoch_nonce,
            lab_nonce: s.lab_nonce,
            last_epoch_block_nonce: s.last_epoch_block_nonce,
            randomness_stabilisation_window: s.randomness_stabilisation_window,
            stability_window_3kf: s.stability_window_3kf,
            genesis_hash: s.genesis_hash,
            rolling_nonce: s.rolling_nonce,
            stability_window: s.stability_window,
            first_block_hash_of_epoch: s.first_block_hash_of_epoch,
            prev_epoch_first_block_hash: s.prev_epoch_first_block_hash,
            pending_pp_updates: s.pending_pp_updates.clone(),
            future_pp_updates: s.future_pp_updates.clone(),
            update_quorum: s.update_quorum,
            governance: s.governance.clone(),
            slot_config: s.slot_config.clone(),
            needs_stake_rebuild: s.needs_stake_rebuild,
            ptr_stake: s.ptr_stake.clone(),
            ptr_stake_excluded: s.ptr_stake_excluded,
            pending_reward_update: s.pending_reward_update.clone(),
            total_stake_key_deposits: s.total_stake_key_deposits,
            script_stake_credentials: s.script_stake_credentials.clone(),
            diff_seq: s.diff_seq.clone(),
            node_network: s.node_network,
            opcert_counters: s.opcert_counters.clone(),
            stake_key_deposits: s.stake_key_deposits.clone(),
            pool_deposits: s.pool_deposits.clone(),
        }
    }
}

impl From<LedgerStateSnapshot> for super::LedgerState {
    fn from(s: LedgerStateSnapshot) -> Self {
        Self {
            utxo_set: s.utxo_set,
            tip: s.tip,
            era: s.era,
            pending_era_transition: s.pending_era_transition,
            epoch: s.epoch,
            epoch_length: s.epoch_length,
            shelley_transition_epoch: s.shelley_transition_epoch,
            byron_epoch_length: s.byron_epoch_length,
            protocol_params: s.protocol_params,
            prev_protocol_params: s.prev_protocol_params,
            prev_d: s.prev_d,
            prev_protocol_version_major: s.prev_protocol_version_major,
            stake_distribution: s.stake_distribution,
            treasury: s.treasury,
            pending_donations: s.pending_donations,
            reserves: s.reserves,
            delegations: s.delegations,
            pool_params: s.pool_params,
            future_pool_params: s.future_pool_params,
            pending_retirements: s.pending_retirements,
            snapshots: s.snapshots,
            reward_accounts: s.reward_accounts,
            pointer_map: s.pointer_map,
            genesis_delegates: s.genesis_delegates,
            epoch_fees: s.epoch_fees,
            epoch_blocks_by_pool: s.epoch_blocks_by_pool,
            epoch_block_count: s.epoch_block_count,
            evolving_nonce: s.evolving_nonce,
            candidate_nonce: s.candidate_nonce,
            epoch_nonce: s.epoch_nonce,
            lab_nonce: s.lab_nonce,
            last_epoch_block_nonce: s.last_epoch_block_nonce,
            randomness_stabilisation_window: s.randomness_stabilisation_window,
            stability_window_3kf: s.stability_window_3kf,
            genesis_hash: s.genesis_hash,
            rolling_nonce: s.rolling_nonce,
            stability_window: s.stability_window,
            first_block_hash_of_epoch: s.first_block_hash_of_epoch,
            prev_epoch_first_block_hash: s.prev_epoch_first_block_hash,
            pending_pp_updates: s.pending_pp_updates,
            future_pp_updates: s.future_pp_updates,
            update_quorum: s.update_quorum,
            governance: s.governance,
            slot_config: s.slot_config,
            needs_stake_rebuild: s.needs_stake_rebuild,
            ptr_stake: s.ptr_stake,
            ptr_stake_excluded: s.ptr_stake_excluded,
            pending_reward_update: s.pending_reward_update,
            total_stake_key_deposits: s.total_stake_key_deposits,
            script_stake_credentials: s.script_stake_credentials,
            diff_seq: s.diff_seq,
            node_network: s.node_network,
            opcert_counters: s.opcert_counters,
            stake_key_deposits: s.stake_key_deposits,
            pool_deposits: s.pool_deposits,
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_ledger_state_snapshot_roundtrip)'`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/snapshot_format.rs
git commit -m "refactor(ledger): add From conversions for LedgerStateSnapshot roundtrip"
```

---

## Task 4: Define EraRules Trait and RuleContext

**Files:**
- Modify: `crates/dugite-ledger/src/eras/mod.rs`

Define the `EraRules` trait, `RuleContext`, and `EraRulesImpl` enum. This is the core abstraction. No implementations yet — just the interface.

- [ ] **Step 1: Replace `eras/mod.rs` with trait definition**

Rewrite `crates/dugite-ledger/src/eras/mod.rs`:

```rust
//! Era-specific ledger transition logic.
//!
//! Each Cardano era introduces new ledger rules while maintaining
//! backward compatibility with previous eras. The `EraRules` trait
//! encapsulates all era-varying behavior, dispatched via `EraRulesImpl`.

pub mod byron;
pub mod common;
pub mod conway;
pub mod shelley;

// These will be added in later tasks:
// pub mod alonzo;
// pub mod babbage;

use std::collections::{HashMap, HashSet};

use dugite_primitives::block::BlockHeader;
use dugite_primitives::era::Era;
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::network::NetworkId;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::Transaction;

use crate::plutus::SlotConfig;
use crate::state::substates::*;
use crate::state::{BlockValidationMode, LedgerError};
use crate::utxo_diff::UtxoDiff;
use dugite_primitives::block::Block;
use dugite_primitives::protocol_params::ProtocolParameters;

/// Read-only context available to all era rules.
/// Assembled by the orchestrator before dispatching.
pub struct RuleContext<'a> {
    pub params: &'a ProtocolParameters,
    pub current_slot: u64,
    pub current_epoch: EpochNo,
    pub era: Era,
    pub slot_config: Option<&'a SlotConfig>,
    pub node_network: Option<NetworkId>,
    pub genesis_delegates: &'a HashMap<Hash28, (Hash28, Hash32)>,
    pub update_quorum: u64,
    pub epoch_length: u64,
    pub shelley_transition_epoch: u64,
    pub byron_epoch_length: u64,
    pub stability_window: u64,
    pub randomness_stabilisation_window: u64,
}

/// Era-specific ledger rules.
///
/// Stateless strategy trait — implementations carry no mutable state.
/// All state lives in the component sub-states passed as parameters.
pub trait EraRules {
    /// Validate block body constraints (ExUnit budgets, ref script sizes).
    fn validate_block_body(
        &self,
        block: &Block,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
    ) -> Result<(), LedgerError>;

    /// Apply a single valid transaction (IsValid=true path).
    ///
    /// Implements the full LEDGER rule pipeline for the era.
    fn apply_valid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
    ) -> Result<UtxoDiff, LedgerError>;

    /// Apply an invalid transaction (IsValid=false, collateral consumption).
    fn apply_invalid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
    ) -> Result<UtxoDiff, LedgerError>;

    /// Process an epoch boundary transition.
    fn process_epoch_transition(
        &self,
        new_epoch: EpochNo,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        epochs: &mut EpochSubState,
        consensus: &mut ConsensusSubState,
    ) -> Result<(), LedgerError>;

    /// Evolve nonce state after a block header.
    fn evolve_nonce(
        &self,
        header: &BlockHeader,
        ctx: &RuleContext,
        consensus: &mut ConsensusSubState,
    );

    /// Minimum fee for a transaction.
    fn min_fee(
        &self,
        tx: &Transaction,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
    ) -> u64;

    /// Handle hard fork state transformations when entering this era.
    fn on_era_transition(
        &self,
        from_era: Era,
        ctx: &RuleContext,
        utxo: &mut UtxoSubState,
        certs: &mut CertSubState,
        gov: &mut GovSubState,
        consensus: &mut ConsensusSubState,
        epochs: &mut EpochSubState,
    ) -> Result<(), LedgerError>;

    /// Compute the set of required VKey witnesses for a transaction.
    fn required_witnesses(
        &self,
        tx: &Transaction,
        ctx: &RuleContext,
        utxo: &UtxoSubState,
        certs: &CertSubState,
        gov: &GovSubState,
    ) -> HashSet<Hash28>;
}
```

- [ ] **Step 2: Create empty `eras/common.rs`**

Create `crates/dugite-ledger/src/eras/common.rs`:

```rust
//! Shared helpers used across multiple era rule implementations.
//!
//! These are NOT on the EraRules trait — they are internal building blocks
//! that era impls compose to avoid duplicating logic. The pattern is
//! composition over inheritance.
```

- [ ] **Step 3: Run tests to verify compilation**

Run: `cargo nextest run -p dugite-ledger --no-tests=pass 2>&1 | head -5`

Expected: Compiles. The trait is defined but not yet implemented by any type. Existing `ByronRules`, `ShelleyLedger`, `ConwayLedger` are unchanged.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/eras/mod.rs crates/dugite-ledger/src/eras/common.rs
git commit -m "refactor(ledger): define EraRules trait and RuleContext

The core abstraction for era-specific ledger rule dispatch. Stateless
strategy pattern with granular sub-state borrows. No implementations
yet — just the interface contract."
```

---

## Task 5: Wire Snapshot Save/Load Through LedgerStateSnapshot

**Files:**
- Modify: `crates/dugite-ledger/src/state/snapshot.rs`

Update the snapshot save/load functions to serialize via `LedgerStateSnapshot` instead of `LedgerState` directly. This prepares for the LedgerState restructuring — once we change LedgerState's field layout, the snapshot format stays stable.

- [ ] **Step 1: Write a bincode roundtrip test**

Add to the test module in `snapshot_format.rs`:

```rust
    #[test]
    fn test_bincode_roundtrip_through_snapshot_format() {
        let state = LedgerState::new(ProtocolParameters::mainnet_defaults());

        // Serialize via snapshot format
        let snapshot = LedgerStateSnapshot::from(&state);
        let bytes = bincode::serialize(&snapshot).expect("serialize");

        // Deserialize back through snapshot format
        let restored_snapshot: LedgerStateSnapshot =
            bincode::deserialize(&bytes).expect("deserialize");
        let restored = LedgerState::from(restored_snapshot);

        // Verify key fields
        assert_eq!(restored.epoch, state.epoch);
        assert_eq!(restored.era, state.era);
        assert_eq!(restored.protocol_params.protocol_version_major,
                   state.protocol_params.protocol_version_major);
    }
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_bincode_roundtrip_through_snapshot_format)'`

Expected: PASS — both `LedgerState` and `LedgerStateSnapshot` use the same field layout with the same serde attributes, so bincode output is identical.

- [ ] **Step 3: Update `save_snapshot()` in `snapshot.rs` to use `LedgerStateSnapshot`**

In `crates/dugite-ledger/src/state/snapshot.rs`, modify the `save_snapshot` function to convert `LedgerState` to `LedgerStateSnapshot` before serializing. Find the `bincode::serialize(self)` call and change it to:

```rust
let snapshot = super::snapshot_format::LedgerStateSnapshot::from(&*self);
let payload = bincode::serialize(&snapshot)
```

Similarly, update `load_snapshot()` to deserialize into `LedgerStateSnapshot` and convert back:

```rust
let snapshot: super::snapshot_format::LedgerStateSnapshot = bincode::deserialize(&payload)?;
// Convert to LedgerState
let state = LedgerState::from(snapshot);
```

Note: The exact edits depend on the current code structure. Read `snapshot.rs` carefully before editing. The key principle is: serialize `LedgerStateSnapshot`, deserialize `LedgerStateSnapshot`, convert to/from `LedgerState`.

- [ ] **Step 4: Run full test suite**

Run: `cargo nextest run -p dugite-ledger`

Expected: All tests pass. The bincode output is byte-identical since the field layout matches.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/snapshot.rs crates/dugite-ledger/src/state/snapshot_format.rs
git commit -m "refactor(ledger): route snapshot save/load through LedgerStateSnapshot

Snapshot serialization now goes through the flat LedgerStateSnapshot
wire format, decoupling the on-disk format from LedgerState's in-memory
layout. This prepares for restructuring LedgerState to use sub-states."
```

---

## Task 6: Restructure LedgerState to Use Sub-States

**Files:**
- Modify: `crates/dugite-ledger/src/state/mod.rs`
- Modify: `crates/dugite-ledger/src/state/apply.rs`
- Modify: `crates/dugite-ledger/src/state/epoch.rs`
- Modify: `crates/dugite-ledger/src/state/certificates.rs`
- Modify: `crates/dugite-ledger/src/state/governance.rs`
- Modify: `crates/dugite-ledger/src/state/rewards.rs`
- Modify: `crates/dugite-ledger/src/state/snapshot_format.rs`
- Modify: `crates/dugite-ledger/src/validation/mod.rs`
- Modify: `crates/dugite-ledger/src/validation/phase1.rs`
- Modify: `crates/dugite-ledger/src/validation/conway.rs`
- Modify: `crates/dugite-ledger/src/validation/collateral.rs`
- Modify: `crates/dugite-ledger/src/ledger_seq.rs`

This is the largest task — replacing `LedgerState`'s flat fields with sub-state components. The approach:
1. Remove `#[derive(Serialize, Deserialize)]` from `LedgerState` (serialization now goes through `LedgerStateSnapshot`)
2. Replace the 52 flat fields with 5 sub-state fields + coordination fields
3. Update all `self.field_name` accesses to `self.substate.field_name`
4. Update the `LedgerStateSnapshot` `From` impls to map between flat and nested layouts

**This task is inherently large.** The agentic worker should:
1. Make the struct change first
2. Use compiler errors to find all broken access sites
3. Fix them mechanically (e.g., `self.utxo_set` → `self.utxo.utxo_set`)
4. Run tests frequently

- [ ] **Step 1: Restructure the `LedgerState` struct**

In `crates/dugite-ledger/src/state/mod.rs`, replace the current `LedgerState` struct definition (lines 75-348) with the sub-state version. Remove `#[derive(Serialize, Deserialize)]` — serialization is handled by `LedgerStateSnapshot`.

```rust
/// Core ledger state, decomposed into component sub-states for granular borrowing.
///
/// Serialization goes through `LedgerStateSnapshot` (see `snapshot_format.rs`).
/// Do NOT derive Serialize/Deserialize on this struct directly.
#[derive(Debug, Clone)]
pub struct LedgerState {
    // Component sub-states (independently borrowable)
    pub utxo: UtxoSubState,
    pub certs: CertSubState,
    pub gov: GovSubState,
    pub consensus: ConsensusSubState,
    pub epochs: EpochSubState,

    // Coordination (immutable config or cross-cutting bookkeeping)
    pub tip: Tip,
    pub era: Era,
    #[allow(dead_code)]
    pub pending_era_transition: Option<(Era, Era, EpochNo)>,
    pub epoch: EpochNo,
    pub epoch_length: u64,
    pub shelley_transition_epoch: u64,
    pub byron_epoch_length: u64,
    pub slot_config: SlotConfig,
    pub genesis_hash: Hash32,
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    pub update_quorum: u64,
    pub node_network: Option<NetworkId>,
    pub stability_window: u64,
    pub randomness_stabilisation_window: u64,
    pub stability_window_3kf: u64,
}
```

- [ ] **Step 2: Update `LedgerState::new()` constructor**

Update the `new()` method to initialize sub-states:

```rust
pub fn new(params: ProtocolParameters) -> Self {
    Self {
        utxo: UtxoSubState {
            utxo_set: UtxoSet::new(),
            diff_seq: DiffSeq::new(),
            epoch_fees: Lovelace(0),
            pending_donations: Lovelace(0),
        },
        certs: CertSubState {
            delegations: Arc::new(HashMap::new()),
            pool_params: Arc::new(HashMap::new()),
            future_pool_params: HashMap::new(),
            pending_retirements: HashMap::new(),
            reward_accounts: Arc::new(HashMap::new()),
            stake_key_deposits: HashMap::new(),
            pool_deposits: HashMap::new(),
            total_stake_key_deposits: 0,
            pointer_map: HashMap::new(),
            stake_distribution: StakeDistributionState { stake_map: HashMap::new() },
            script_stake_credentials: HashSet::new(),
        },
        gov: GovSubState {
            governance: Arc::new(GovernanceState::default()),
        },
        consensus: ConsensusSubState {
            evolving_nonce: Hash32::ZERO,
            candidate_nonce: Hash32::ZERO,
            epoch_nonce: Hash32::ZERO,
            lab_nonce: Hash32::ZERO,
            last_epoch_block_nonce: Hash32::ZERO,
            rolling_nonce: Hash32::ZERO,
            first_block_hash_of_epoch: None,
            prev_epoch_first_block_hash: None,
            epoch_blocks_by_pool: Arc::new(HashMap::new()),
            epoch_block_count: 0,
            opcert_counters: HashMap::new(),
        },
        epochs: EpochSubState {
            snapshots: EpochSnapshots::default(),
            treasury: Lovelace(0),
            reserves: Lovelace(MAX_LOVELACE_SUPPLY),
            pending_reward_update: None,
            pending_pp_updates: BTreeMap::new(),
            future_pp_updates: BTreeMap::new(),
            needs_stake_rebuild: false,
            ptr_stake: HashMap::new(),
            ptr_stake_excluded: false,
            protocol_params: params,
            prev_protocol_params: ProtocolParameters::mainnet_defaults(),
            prev_protocol_version_major: 7,
            prev_d: 1.0,
        },
        tip: Tip::origin(),
        era: Era::Conway,
        pending_era_transition: None,
        epoch: EpochNo(0),
        epoch_length: 432000,
        shelley_transition_epoch: 208,
        byron_epoch_length: 21600,
        slot_config: SlotConfig::default(),
        genesis_hash: Hash32::ZERO,
        genesis_delegates: HashMap::new(),
        update_quorum: 5,
        node_network: None,
        stability_window: 0,
        randomness_stabilisation_window: 172800,
        stability_window_3kf: 129600,
    }
}
```

- [ ] **Step 3: Update `LedgerStateSnapshot` `From` impls**

Update the `From` impls in `snapshot_format.rs` to map between flat (snapshot) and nested (LedgerState) layouts. The `From<&LedgerState>` impl now reads from sub-states:

```rust
// Example: s.utxo_set becomes s.utxo.utxo_set
// s.protocol_params becomes s.epochs.protocol_params
// s.treasury becomes s.epochs.treasury
// s.delegations becomes s.certs.delegations
// s.evolving_nonce becomes s.consensus.evolving_nonce
// s.governance becomes s.gov.governance
```

Similarly, `From<LedgerStateSnapshot> for LedgerState` distributes flat fields into sub-states.

- [ ] **Step 4: Fix all field access sites using compiler errors**

Run `cargo build -p dugite-ledger 2>&1 | head -100` and fix errors iteratively. The mechanical mapping is:

| Old access | New access |
|-----------|-----------|
| `self.utxo_set` | `self.utxo.utxo_set` |
| `self.diff_seq` | `self.utxo.diff_seq` |
| `self.epoch_fees` | `self.utxo.epoch_fees` |
| `self.pending_donations` | `self.utxo.pending_donations` |
| `self.delegations` | `self.certs.delegations` |
| `self.pool_params` | `self.certs.pool_params` |
| `self.future_pool_params` | `self.certs.future_pool_params` |
| `self.pending_retirements` | `self.certs.pending_retirements` |
| `self.reward_accounts` | `self.certs.reward_accounts` |
| `self.stake_key_deposits` | `self.certs.stake_key_deposits` |
| `self.pool_deposits` | `self.certs.pool_deposits` |
| `self.total_stake_key_deposits` | `self.certs.total_stake_key_deposits` |
| `self.pointer_map` | `self.certs.pointer_map` |
| `self.stake_distribution` | `self.certs.stake_distribution` |
| `self.script_stake_credentials` | `self.certs.script_stake_credentials` |
| `self.governance` | `self.gov.governance` |
| `self.evolving_nonce` | `self.consensus.evolving_nonce` |
| `self.candidate_nonce` | `self.consensus.candidate_nonce` |
| `self.epoch_nonce` | `self.consensus.epoch_nonce` |
| `self.lab_nonce` | `self.consensus.lab_nonce` |
| `self.last_epoch_block_nonce` | `self.consensus.last_epoch_block_nonce` |
| `self.rolling_nonce` | `self.consensus.rolling_nonce` |
| `self.first_block_hash_of_epoch` | `self.consensus.first_block_hash_of_epoch` |
| `self.prev_epoch_first_block_hash` | `self.consensus.prev_epoch_first_block_hash` |
| `self.epoch_blocks_by_pool` | `self.consensus.epoch_blocks_by_pool` |
| `self.epoch_block_count` | `self.consensus.epoch_block_count` |
| `self.opcert_counters` | `self.consensus.opcert_counters` |
| `self.snapshots` | `self.epochs.snapshots` |
| `self.treasury` | `self.epochs.treasury` |
| `self.reserves` | `self.epochs.reserves` |
| `self.pending_reward_update` | `self.epochs.pending_reward_update` |
| `self.pending_pp_updates` | `self.epochs.pending_pp_updates` |
| `self.future_pp_updates` | `self.epochs.future_pp_updates` |
| `self.needs_stake_rebuild` | `self.epochs.needs_stake_rebuild` |
| `self.ptr_stake` | `self.epochs.ptr_stake` |
| `self.ptr_stake_excluded` | `self.epochs.ptr_stake_excluded` |
| `self.protocol_params` | `self.epochs.protocol_params` |
| `self.prev_protocol_params` | `self.epochs.prev_protocol_params` |
| `self.prev_protocol_version_major` | `self.epochs.prev_protocol_version_major` |
| `self.prev_d` | `self.epochs.prev_d` |

Also update any external crate access (e.g., `dugite-node`, `dugite-cli`, `dugite-mempool`) that accesses `LedgerState` fields directly. Use `cargo build --workspace 2>&1 | grep "error"` to find all broken sites across the workspace.

- [ ] **Step 5: Update public re-exports in `lib.rs`**

Ensure `crates/dugite-ledger/src/lib.rs` re-exports the sub-state types:

```rust
pub use state::substates::{
    CertSubState, ConsensusSubState, EpochSubState, GovSubState, UtxoSubState,
};
```

- [ ] **Step 6: Run full workspace tests**

Run: `cargo nextest run --workspace`

Expected: All tests pass. This is a pure field-access refactor — no behavior changes.

- [ ] **Step 7: Run clippy and fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`

Expected: Clean.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "refactor(ledger): restructure LedgerState into component sub-states

Replace 52 flat fields with 5 sub-state components:
- UtxoSubState: utxo_set, diff_seq, epoch_fees, pending_donations
- CertSubState: delegations, pool_params, reward_accounts, etc.
- GovSubState: governance
- ConsensusSubState: nonces, block counters, opcert tracking
- EpochSubState: snapshots, treasury, reserves, protocol_params

Serialization goes through LedgerStateSnapshot flat wire format.
All field accesses updated across the workspace."
```

---

## Task 7: Implement ByronRules

**Files:**
- Modify: `crates/dugite-ledger/src/eras/byron.rs`
- Modify: `crates/dugite-ledger/src/eras/mod.rs`

Implement `EraRules` for `ByronRules`. Byron is the simplest era (no scripts, no certificates, no governance, OBFT nonce) and already has dedicated logic in the existing `byron.rs`. This is the first era wired through the trait.

- [ ] **Step 1: Write a test for Byron block application through the trait**

Add to `crates/dugite-ledger/src/eras/byron.rs` (in the existing `tests` module):

```rust
#[test]
fn test_byron_rules_validate_block_body_accepts_valid() {
    use crate::eras::{EraRules, RuleContext};
    use crate::state::substates::*;

    let rules = ByronRules;
    let params = ProtocolParameters::mainnet_defaults();
    let ctx = RuleContext {
        params: &params,
        current_slot: 0,
        current_epoch: EpochNo(0),
        era: Era::Byron,
        slot_config: None,
        node_network: None,
        genesis_delegates: &HashMap::new(),
        update_quorum: 5,
        epoch_length: 21600,
        shelley_transition_epoch: 208,
        byron_epoch_length: 21600,
        stability_window: 0,
        randomness_stabilisation_window: 0,
    };
    let utxo = UtxoSubState {
        utxo_set: UtxoSet::new(),
        diff_seq: DiffSeq::new(),
        epoch_fees: Lovelace(0),
        pending_donations: Lovelace(0),
    };

    // Byron has no block-body ExUnit checks — validate_block_body is a no-op
    // We can't easily construct a full Block here, so this test verifies
    // the trait is implemented and callable.
    // Full integration testing comes via the existing apply_block tests.
}
```

- [ ] **Step 2: Implement `EraRules` for `ByronRules`**

In `crates/dugite-ledger/src/eras/byron.rs`, add the trait implementation. Byron's rules are simple:
- `validate_block_body`: no ExUnit or ref-script checks — return `Ok(())`
- `apply_valid_tx`: use existing `apply_byron_block` / `validate_byron_tx` logic
- `apply_invalid_tx`: Byron has no IsValid concept — return error
- `process_epoch_transition`: minimal — snapshot rotation, no governance, no DRep pulser
- `evolve_nonce`: OBFT — lab_nonce = prev_hash, evolving_nonce does NOT advance
- `min_fee`: `min_fee_a * tx_size + min_fee_b`
- `on_era_transition`: Byron is the first era — no transition needed
- `required_witnesses`: spending input keys only (no scripts, no certs)

The implementation should delegate to the existing `apply_byron_block`, `validate_byron_tx`, and `ByronFeePolicy` functions already in `byron.rs`.

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p dugite-ledger`

Expected: All pass including the new test.

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/eras/byron.rs crates/dugite-ledger/src/eras/mod.rs
git commit -m "refactor(ledger): implement EraRules for ByronRules

First era wired through the trait. Byron rules: no ExUnit checks,
OBFT nonce (no VRF), min_fee = a*size + b, no governance/scripts."
```

---

## Tasks 8-11: Implement Remaining Era Rules

These tasks follow the same pattern as Task 7. Each era composes shared helpers from `common.rs` with era-specific logic.

### Task 8: Extract Shared Helpers into `common.rs`

**Files:**
- Modify: `crates/dugite-ledger/src/eras/common.rs`

Extract the following reusable functions from `state/apply.rs`, `state/certificates.rs`, `validation/mod.rs`:
- `validate_shelley_base()` — Phase-1 rules 1-10 (inputs, fees, sizes, TTL)
- `apply_utxo_changes()` — consume inputs, produce outputs, record diff
- `apply_collateral_consumption()` — IsValid=false collateral path
- `process_shelley_certs()` — Shelley cert types (registration, deregistration, delegation, pool reg/retire)
- `drain_withdrawal_accounts()` — subtract withdrawal amounts from reward accounts
- `compute_shelley_nonce()` — VRF-based nonce evolution (shared Shelley+)

Each function takes sub-state references as parameters instead of `&mut self`.

### Task 9: Implement ShelleyRules

**Files:**
- Modify: `crates/dugite-ledger/src/eras/shelley.rs`

Replace the empty `ShelleyLedger` marker struct with `ShelleyRules` implementing `EraRules`. Covers Shelley, Allegra, and Mary eras. Uses `common::` helpers for all validation and cert processing. Epoch transition: SNAP + POOLREAP + pre-Conway PP update proposals.

### Task 10: Implement AlonzoRules and BabbageRules

**Files:**
- Create: `crates/dugite-ledger/src/eras/alonzo.rs`
- Create: `crates/dugite-ledger/src/eras/babbage.rs`

`AlonzoRules` adds: Phase-2 Plutus evaluation, collateral validation, IsValid=false path, script data hash. `BabbageRules` adds: reference inputs, inline datums, collateral return, `babbageUtxoValidation` (which Conway reuses).

### Task 11: Implement ConwayRules

**Files:**
- Modify: `crates/dugite-ledger/src/eras/conway.rs`

The largest implementation. Replace the empty `ConwayLedger` marker struct with `ConwayRules`:
- Full 9-step LEDGER pipeline (treasury validation, ref-script size, DRep withdrawal delegation, DRep expiry, drain accounts, CERTS with GOVCERT, GOV, UTXOW)
- Epoch transition: 13-step Conway EPOCH (SNAP, POOLREAP, ratification/enactment, treasury withdrawals, proposal lifecycle, DRep pulser, totalObligation recalculation)
- TranslateEra from Babbage: pointer stake purge, VState creation, VRF key map, ConwayGovState init
- Tiered ref-script fees
- Voter witness requirements

---

## Task 12: Rewrite `apply_block` as Thin Orchestrator

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs`

Replace the monolithic `apply_block()` with the orchestrator pattern from the spec:

```rust
pub fn apply_block(&mut self, block: &Block, mode: BlockValidationMode) -> Result<(), LedgerError> {
    let rules = EraRulesImpl::for_era(block.era);
    let ctx = RuleContext::from_state(self, block);

    self.verify_block_connects(block)?;

    if block.era > self.era {
        rules.on_era_transition(self.era, &ctx, ...)?;
        self.pending_era_transition = Some((self.era, block.era, self.epoch));
    }

    // Epoch transitions
    let block_epoch = EpochNo(self.epoch_of_slot(block.slot().0));
    while self.epoch < block_epoch {
        let next = EpochNo(self.epoch.0 + 1);
        let epoch_rules = EraRulesImpl::for_era(self.era);
        epoch_rules.process_epoch_transition(next, &ctx, ...)?;
        self.epoch = next;
    }

    rules.validate_block_body(block, &ctx, &self.utxo)?;

    let mut block_diff = UtxoDiff::new();
    for tx in &block.transactions {
        let diff = if tx.is_valid {
            rules.apply_valid_tx(tx, mode, &ctx, ...)?
        } else {
            rules.apply_invalid_tx(tx, mode, &ctx, &mut self.utxo)?
        };
        block_diff.merge(diff);
    }

    rules.evolve_nonce(&block.header, &ctx, &mut self.consensus);

    self.tip = block.tip();
    self.era = block.era;
    self.utxo.diff_seq.push(block.slot(), *block.hash(), block_diff);

    Ok(())
}
```

The old `apply_block` code is deleted — all logic now lives in era rule impls.

---

## Task 13: Clean Up

**Files:**
- Delete unused code from `state/apply.rs` (old inline era dispatch)
- Delete empty `ShelleyLedger`, `ConwayLedger` marker structs if still present
- Remove `#[allow(dead_code)]` attributes added during migration
- Remove any remaining `protocol_version_major` guards outside of era rule impls
- Run full workspace test suite + clippy + fmt
- Verify: `cargo nextest run --workspace && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`

---

## Verification Checklist

After all tasks complete:

- [ ] `cargo nextest run --workspace` — all tests pass
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt --all -- --check` — clean
- [ ] `cargo build --release` — builds successfully
- [ ] No `protocol_version_major >=` checks remain outside of `eras/` module (verify with `grep -r "protocol_version_major" crates/dugite-ledger/src/ --include="*.rs" | grep -v test | grep -v eras/`)
- [ ] Snapshot roundtrip: save with old code, load with new code (or vice versa) — verify compatible via the `LedgerStateSnapshot` wire format
- [ ] All 7 invariants from the spec have at least one test asserting them
