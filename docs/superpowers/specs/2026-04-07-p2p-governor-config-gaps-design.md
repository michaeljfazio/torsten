# Design: P2P Governor and Config Gaps vs Haskell cardano-node 10.x

**Issue:** #369  
**Date:** 2026-04-07  
**Status:** Draft

## Overview

Close the remaining gaps between Dugite's P2P governor, topology handling, and config parsing compared to Haskell cardano-node 10.x. Our governor already has per-group local root promotion (`governor.rs` lines 152-233) and topology demotion exclusions in selection functions (`select_worst_hot`, `select_worst_warm`, `select_lowest_reputation_cold`). The remaining work is: ensuring the aggregate target paths exclude topology peers, adding the `aboveTargetLocal` exception path, wiring per-group `diffusionMode` to handshakes, adding missing config fields, and startup validation. This spec covers 7 work items ordered by priority.

> **Note on peer selection strategy:** Haskell uses **uniform random selection** for all promotions (`simplePromotionPolicy`) and score-based selection only for hot demotions. Our implementation uses score-based selection throughout. This is a deliberate divergence — our scoring approach (reputation + latency + failure penalty) provides better peer quality optimization while maintaining the same structural guarantees (per-group targets, topology exclusions). This spec does not change the selection strategy.

---

## 1. Governor: `below_target_local` — Per-Group Local Root Promotion

**Priority:** High

### Haskell Behaviour

In Haskell, `belowTargetLocal` in `EstablishedPeers.hs` and `ActivePeers.hs` operates as an **independent guard** evaluated before `belowTargetOther`. It iterates each local root group independently:

**Cold-to-warm (`EstablishedPeers.belowTargetLocal`):**

1. For each group in `LocalRootPeers.toGroupSets` (preserving topology order):
   - Compute `membersEstablished = group.members ∩ establishedPeers`
   - If `|membersEstablished| < warmValency` → group is deficient
2. For each deficient group:
   - `availableToPromote = localAvailableToConnect \ localEstablishedPeers \ localConnectInProgress \ inProgressDemoteToCold`
   - `membersAvailableToPromote = group.members ∩ availableToPromote` — **strictly from this group's members**
   - `numToPromote = warmTarget - |membersEstablished| - |connectInProgress ∩ group.members|`
   - Pick `numToPromote` peers from `membersAvailableToPromote`
3. Union all selected peers across groups → emit `PromoteColdPeer` jobs

**Warm-to-hot (`ActivePeers.belowTargetLocal`):**

Same pattern but checks `|membersActive| < hotValency` per group. An additional constraint: peers must pass `readyPeers` — a peer is only eligible for warm→hot if it is NOT in `nextActivateTimes` (the repromote delay queue). A peer lands in `nextActivateTimes` when it suffers an async demotion from hot back to warm; it must wait `repromoteDelay` before being eligible again. Note: `policyPeerShareActivationDelay` (default 300s) is a **separate** concept controlling when a peer can participate in peer sharing, NOT when it can be promoted to hot. A newly-warmed peer is immediately eligible for hot promotion unless it was recently demoted from hot.

**Key properties:**
- Fires even when aggregate `targetNumberOfEstablishedPeers` is satisfied
- `belowTargetOther` explicitly excludes `LocalRootPeers.keysSet` from its candidate set — local roots are ONLY promoted by `belowTargetLocal`
- Guard ordering: `belowTargetBigLedgerPeers <> belowTargetLocal <> belowTargetOther` — local has priority via STM `<|>` (orElse) semantics

### Current State

`compute_actions()` in `governor.rs` **already has** per-group local root promotion (Stage 1, lines 152-233). It iterates each `LocalRootGroupTarget`, counts warm-or-hot members, and promotes cold→warm / warm→hot for deficient groups. It also uses an `already_promoted` HashSet to prevent double-promoting.

### Remaining Gap

1. **Aggregate target paths don't exclude topology peers.** Stage 2 (lines 237-255) and Stage 3 (lines 257-273) can promote topology peers again via the aggregate path, duplicating the per-group logic. Haskell's `belowTargetOther` explicitly excludes `LocalRootPeers.keysSet` — our aggregate paths must do the same.

2. **No `inProgressPromoteCold` tracking.** Haskell tracks a global `Set peeraddr` of peers with in-flight cold→warm promotion jobs. This prevents double-promotion across governor iterations when async promotion is slow. Our `already_promoted` HashSet is per-invocation only — it doesn't persist across ticks.

3. **No backoff-aware filtering in local root promotion.** Haskell's `localAvailableToConnect` is `localRootPeers ∩ KnownPeers.availableToConnect` — peers in exponential backoff after connection failure are excluded. Our per-group promotion (Stage 1) uses `peers_eligible_to_connect()` which does filter by backoff, but verify the intersection is correct.

### Design

1. **Exclude topology peers from aggregate paths:** In Stage 2 (`below_target_other_warm`), filter out `PeerSource::Topology` from the eligible set. In Stage 3 (`below_target_other_hot`), same exclusion. This ensures local roots are ONLY managed by the per-group guards.

2. **Add `in_progress_promote_cold: HashSet<SocketAddr>` to `Governor`:** Populated when `PromoteToWarm` actions are emitted, cleared when the promotion completes (success or failure callback). Subtract from candidate sets in both local and aggregate paths. The `numToPromote` calculation per group should subtract `|inProgressPromoteCold ∩ group.members|`.

3. **Verify backoff filtering:** Ensure Stage 1 correctly intersects group members with `peers_eligible_to_connect()` results (peers whose `next_connect_after` has elapsed or is None).

### Acceptance Criteria

- A BP with 3 relays in one group (`warmValency: 3`) always reconnects within one governor tick when a relay disconnects, even if aggregate peer targets are met
- Multiple local root groups tracked independently
- Unit test: governor with 2 groups, one below target, emits `PromoteToWarm` only for the deficient group's members
- Unit test: `belowTargetOther` never selects topology peers

---

## 2. Governor: Local Root Demotion Exclusion

**Priority:** High

### Haskell Behaviour

Local root peers are excluded from **every** demotion and forget candidate set via `Set.\\ LocalRootPeers.keysSet localRootPeers`. Complete inventory:

| Site | Function | Exclusion |
|------|----------|-----------|
| warm→cold | `EstablishedPeers.aboveTargetOther` | `Set.\\ LocalRootPeers.keysSet` — unconditional |
| hot→warm (non-big-ledger) | `ActivePeers.aboveTargetOther` | `Set.\\ LocalRootPeers.keysSet` — unconditional |
| hot→warm (big-ledger) | `ActivePeers.aboveTargetBigLedgerPeers` | `Set.\\ LocalRootPeers.keysSet` — unconditional |
| hot→warm (local root) | `ActivePeers.aboveTargetLocal` | **Exception**: fires when a group EXCEEDS its `hotValency` — this is the only path that can demote local roots |
| forget cold | `KnownPeers.aboveTarget` | `Set.\\ LocalRootPeers.keysSet` — unconditional (invariant: localRootPeers ⊆ knownPeers) |

**The one exception:** `aboveTargetLocal` in `ActivePeers.hs` can demote local root peers when a specific group has MORE active members than its `hotValency`. This handles cases where inbound connections have pushed a group over its configured hot target. The logic:
1. Find groups where `|members ∩ activePeers| > hotValency`
2. Select excess peers from that group's active members
3. Emit `DemoteToWarm` for the excess

**Churn never targets local roots:** Churn modifies `PeerSelectionTargets` (the target numbers), not individual peers. When targets decrease, `aboveTargetOther` fires — but local roots are excluded from those candidate sets. Additionally, `decreaseActivePeers` floors the active target at `max 1 localRootHotTarget` to avoid dropping below local root needs.

### Current State

Our code **already excludes** topology peers from:
- `select_worst_hot()` (line 58 in selection.rs) — hot→warm demotion
- `select_worst_warm()` (line 111) — warm→cold demotion
- `select_lowest_reputation_cold()` (line 82) — cold churn eviction
- `peer_failed()` (networking.rs line 398-419) — forgetting after 5 failures

### Remaining Gap

1. **Stage 4 in `compute_actions()` (lines 275-300)** — the hot→warm target-driven demotion path. This path **does** filter topology peers (lines 288-290 check `info.source == PeerSource::Topology`), so it's already correct. **Verify this is consistent.**

2. **Missing `aboveTargetLocal` exception path.** There is no logic to demote local root peers when a group exceeds its `hotValency`. This can happen when inbound connections push a group above target.

3. **No `aboveTargetLocal` in EstablishedPeers.** Confirmed: Haskell has NO `aboveTargetLocal` for warm→cold. Local root warm peers are **never** demoted to cold. Only `ActivePeers.aboveTargetLocal` exists (hot→warm when group exceeds `hotValency`).

### Design

1. **Audit and confirm existing exclusions.** Verify all demotion paths in `compute_actions()` and all selection functions exclude topology peers. The code exploration shows this is already done for the selection functions.

2. **New: `select_excess_local_hot(peer_manager, groups) -> Vec<SocketAddr>`**: For each local root group where `|hot_members| > hot_valency`, select `excess` worst-scoring hot members for demotion. This is the one exception path.

In `governor.rs`, update the above-target logic:

```
above_target_hot:
  1. above_target_local_hot() → demote excess in oversubscribed local groups
  2. above_target_other_hot() → demote from non-local-root hot peers only
  
above_target_warm:
  1. above_target_other_warm() → demote from non-local-root warm peers only
  (no above_target_local_warm — Haskell doesn't have this; warm local roots are never demoted)
```

### Acceptance Criteria

- Topology peers never emitted as `DemoteToWarm` or `DemoteToCold` targets (except the `aboveTargetLocal` path when group exceeds `hotValency`)
- Topology peers never forgotten regardless of failure count or churn
- Unit test: governor with topology + ledger peers above target only demotes ledger peers
- Unit test: local root group with hot members > hotValency → excess members demoted

---

## 3. Missing P2P Config Fields

**Priority:** Medium

### Fields to Add to `NodeConfig`

#### Genesis / Consensus Mode

| JSON Key | Type | Default | Purpose |
|----------|------|---------|---------|
| `ConsensusMode` | enum `PraosMode \| GenesisMode` | `PraosMode` | Enables Ouroboros Genesis |
| `SyncTargetNumberOfActivePeers` | `usize` | `5` | Active peers during Genesis sync |
| `SyncTargetNumberOfEstablishedPeers` | `usize` | `10` | Established peers during sync |
| `SyncTargetNumberOfKnownPeers` | `usize` | `150` | Known peers during sync |
| `SyncTargetNumberOfRootPeers` | `usize` | `0` | Root peers during sync |
| `SyncTargetNumberOfActiveBigLedgerPeers` | `usize` | `30` | Active BLPs during sync |
| `SyncTargetNumberOfEstablishedBigLedgerPeers` | `usize` | `40` | Established BLPs during sync |
| `SyncTargetNumberOfKnownBigLedgerPeers` | `usize` | `100` | Known BLPs during sync |
| `MinBigLedgerPeersForTrustedState` | `usize` | `5` | Pause sync if active BLPs drop below this |

> **Source:** `defaultSyncTargets` in `cardano-diffusion/lib/Cardano/Network/Diffusion/Configuration.hs` lines 51-60. Note `SyncTargetNumberOfEstablishedBigLedgerPeers` defaults to 40 (not 50 as stated in the issue).

**Haskell `ConsensusMode` type:** Parsed from JSON string literals `"PraosMode"` or `"GenesisMode"` via derived `FromJSON`. Default is `PraosMode`. In `GenesisMode`, the governor switches between "deadline targets" (normal) and "sync targets" (when `LedgerStateJudgement = TooOld`). When `PraosMode`, sync targets are ignored.

#### Connection Management

| JSON Key | Type | Default | Purpose |
|----------|------|---------|---------|
| `AcceptedConnectionsLimit` | struct | `{ hard: 512, soft: 384, delay: 5.0 }` | Inbound connection limits |
| `ProtocolIdleTimeout` | `f64` (seconds) | `5.0` | Idle mini-protocol pruning |
| `TimeWaitTimeout` | `f64` (seconds) | `60.0` | Connection TIME_WAIT duration |
| `EgressPollInterval` | `f64` (seconds) | `0.0` | Outbound governor poll interval |
| `ChainSyncIdleTimeout` | `Option<f64>` (seconds) | `None` (means use default 3373s) | ChainSync-specific idle timeout |

**`AcceptedConnectionsLimit` JSON format:** Parsed via a hand-written `FromJSON` instance in `OrphanInstances.hs` (lines 284-305). The JSON keys are **short forms**, NOT the full Haskell field names:

```json
"AcceptedConnectionsLimit": {
  "hardLimit": 512,
  "softLimit": 384,
  "delay": 5
}
```

JSON keys: `"hardLimit"` (Word32), `"softLimit"` (Word32), `"delay"` (DiffTime/seconds). The delay is NOT fixed — it scales linearly from 0 at soft limit to `delay` at hard limit. Above hard limit: block until count drops below, then wait `delay`.

### Design

Add a `ConsensusMode` enum and `AcceptedConnectionsLimit` struct to `config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusMode {
    PraosMode,
    GenesisMode,
}

/// Matches Haskell JSON format: {"hardLimit": 512, "softLimit": 384, "delay": 5}
/// Hand-written FromJSON in OrphanInstances.hs uses short key names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceptedConnectionsLimit {
    #[serde(rename = "hardLimit")]
    pub hard_limit: u32,
    #[serde(rename = "softLimit")]
    pub soft_limit: u32,
    #[serde(rename = "delay")]
    pub delay: f64,
}
```

Add all fields to `NodeConfig` with `#[serde(default)]` using Haskell-matching defaults. All config JSON keys are **PascalCase** (matching Haskell `.:?` key strings from `POM.hs`). **Phase 1** (this issue): parse and store. **Phase 2** (future): wire `AcceptedConnectionsLimit` to the N2N listener, Genesis targets to the governor.

### Acceptance Criteria

- All fields parse from cardano-node config JSON without errors
- Missing fields use Haskell-matching defaults
- `ConsensusMode` accepts exactly `"PraosMode"` and `"GenesisMode"`
- `AcceptedConnectionsLimit` parses as nested object with short keys (`hardLimit`, `softLimit`, `delay`)
- `ChainSyncIdleTimeout`: `None` in config → use default 3373s (same for all modes, not mode-specific)
- Unit tests for parsing valid configs and verifying defaults

---

## 4. Topology: Per-Group `diffusionMode` Wiring

**Priority:** Medium

### Haskell Behaviour

The `rootDiffusionMode` field in `LocalRootPeersGroup` propagates through a 5-step chain:

1. **Topology parse** → `LocalRootConfig { diffusionMode }` stored per peer
2. **Governor** → `belowTargetLocal` reads `localProvenance` / `diffusionMode` when promoting cold peers
3. **PeerStateActions** → `establishPeerConnection` passes `diffusionMode` to connection manager
4. **ConnectionManager** → `acquireOutboundConnectionImpl` calls `updateVersionData diffusionMode` to override per-connection
5. **Handshake** → CBOR-encodes `InitiatorOnlyDiffusionMode = True`, `InitiatorAndResponderDiffusionMode = False`

Negotiation uses `min(local, remote)` — `InitiatorOnly < InitiatorAndResponder`, so either side requesting initiator-only makes the connection unidirectional.

### Current State

- `topology.rs` already parses `diffusion_mode: Option<String>` on `LocalRootGroup`
- `networking.rs` already stores `diffusion_mode: Option<DiffusionMode>` in `LocalRootGroupInfo` (line 131)
- `effective_diffusion_mode(&self, addr: &SocketAddr) -> DiffusionMode` already exists in `NodePeerManager` (lines 655-664) — checks group membership, applies per-group override, falls back to node-level config

### Remaining Gap

The `effective_diffusion_mode()` method exists but is **not called** from `connection_lifecycle.rs` during outbound connection establishment. All outbound handshakes use the global `DiffusionMode`.

### Design

1. In `connection_lifecycle.rs`, when establishing an outbound connection to a topology peer, call `effective_diffusion_mode(addr)` on `NodePeerManager` to determine the handshake `initiator_only` flag instead of using the global config
2. Verify the CBOR encoding: `InitiatorOnlyDiffusionMode` → `True`, `InitiatorAndResponderDiffusionMode` → `False` (matching Haskell wire format)

**JSON values:** `"InitiatorOnly"` and `"InitiatorAndResponder"` (matching Haskell constructor names). Default when omitted: `InitiatorAndResponder`.

### Acceptance Criteria

- Topology JSON with `"diffusionMode": "InitiatorOnly"` on a group → handshake sends `initiator_only = true`
- Default (field omitted) → `InitiatorAndResponder` (full duplex)
- Unit test: parse topology with mixed diffusion modes across groups
- Unit test: `peer_diffusion_mode()` returns correct mode per group membership

---

## 5. Topology: Additional Fields

**Priority:** Low

### 5a. `behindFirewall` / Provenance

**Haskell behaviour:** The topology field is parsed as a bool (`"behindFirewall": true`). When `true`, the peer's `Provenance` is set to `Inbound`; when `false` (default), `Outbound`.

**Effect:** The governor does NOT skip outbound connection attempts for `Inbound` provenance peers. Instead, when `acquireOutboundConnectionImpl` sees `Provenance = Inbound` and finds an existing inbound connection in `UnnegotiatedState Inbound`, it reuses that connection rather than opening a new outbound one. The provenance is an optimization hint, not a hard gate.

**Current state:** `topology.rs` already parses `behind_firewall: Option<bool>`. Not wired to connection logic.

**Design:** Store the provenance in `LocalRootGroupInfo`. When promoting a cold peer from a `behind_firewall` group, tag the connection attempt with `Provenance::Inbound` so the connection lifecycle manager can opportunistically reuse an existing inbound connection. This is a minor optimization — implement as a future enhancement after the core governor changes.

### 5b. `peerSnapshotFile`

**Haskell behaviour:** A file path (relative to topology directory) containing a JSON snapshot of big ledger peers. Three format versions (V1, V2, V23). Loaded on startup, peers injected into the known peer set for faster bootstrap.

**Current state:** `topology.rs` already parses `peer_snapshot_file: Option<String>`. Not loaded.

**Design:** Parse-only for now. Loading peer snapshots is a Genesis mode feature — implement when Genesis mode is wired.

### 5c. `advertise`

**Current state:** Already parsed in `LocalRootGroup`. `is_advertisable(&self, addr: &SocketAddr)` exists in `NodePeerManager` (lines 677-684) — checks group membership for `advertise` flag, defaults `true` for non-topology peers.

**Gap:** `is_advertisable()` is NOT integrated into the peer sharing response path. The governor's `PeerShareRequest` action (line 355-360 in governor.rs) randomly picks from ALL warm peers with `peer_sharing=true` — it doesn't filter by `advertise`.

**Design:** When building the peer sharing response, filter out peers where `is_advertisable()` returns `false`.

### Acceptance Criteria

- `behindFirewall` and `peerSnapshotFile` parse without errors (already done)
- `advertise` exclusion from peer sharing verified and tested
- No functional changes to connection behaviour in this iteration

---

## 6. Startup Validation: Peer Target Constraints

**Priority:** Medium

### Haskell Behaviour

`sanePeerSelectionTargets` in `Governor/Types.hs` enforces:

```
0 <= active <= established <= known
0 <= rootPeers <= known
active <= 100
established <= 1000
known <= 10000
```

Applied independently to both deadline targets and sync targets (when `GenesisMode`). Called from `makeNodeConfiguration` — node fails to start with an explicit error message if violated.

### Design

Add `validate_peer_targets()` to `NodeConfig`:

```rust
impl NodeConfig {
    pub fn validate_peer_targets(&self) -> Result<(), String> {
        Self::check_targets(
            "deadline",
            self.target_number_of_active_peers,
            self.target_number_of_established_peers,
            self.target_number_of_known_peers,
            self.target_number_of_root_peers,
        )?;
        Self::check_targets_blp(
            "deadline BLP",
            self.target_number_of_active_big_ledger_peers,
            self.target_number_of_established_big_ledger_peers,
            self.target_number_of_known_big_ledger_peers,
        )?;
        if self.consensus_mode == ConsensusMode::GenesisMode {
            Self::check_targets(
                "sync",
                self.sync_target_number_of_active_peers,
                self.sync_target_number_of_established_peers,
                self.sync_target_number_of_known_peers,
                self.sync_target_number_of_root_peers,
            )?;
            Self::check_targets_blp(
                "sync BLP",
                self.sync_target_number_of_active_big_ledger_peers,
                self.sync_target_number_of_established_big_ledger_peers,
                self.sync_target_number_of_known_big_ledger_peers,
            )?;
        }
        Ok(())
    }

    fn check_targets(label: &str, active: usize, established: usize, known: usize, root: usize) -> Result<(), String> {
        // 0 <= active <= established <= known
        // 0 <= root <= known
        // active <= 100, established <= 1000, known <= 10000
    }

    fn check_targets_blp(label: &str, active: usize, established: usize, known: usize) -> Result<(), String> {
        // Same chain: 0 <= active <= established <= known
        // active <= 100, established <= 1000, known <= 10000
    }
}
```

Call from `node/mod.rs` during startup, before network initialization. Exit with clear error message matching Haskell's format.

### Acceptance Criteria

- Node refuses to start when `known < established` or `established < active`
- Upper bounds enforced: active ≤ 100, established ≤ 1000, known ≤ 10000
- Root peers validated: `root ≤ known`
- Validation covers both deadline and sync target sets (sync only when `GenesisMode`)
- Unit tests for valid and invalid configurations
- Default config values pass validation

---

## 7. dugite-config: Schema Alignment

**Priority:** Medium

### Design

Update `crates/dugite-config/src/schema.rs` to add `ParamDef` entries for every new config field. Group them by section:

**Network section (existing):**
- `ConsensusMode` — `ParamType::Enum { values: &["PraosMode", "GenesisMode"] }`
- `ProtocolIdleTimeout` — `ParamType::U64 { min: 0, max: 3600 }` (seconds)
- `TimeWaitTimeout` — `ParamType::U64 { min: 0, max: 3600 }`
- `EgressPollInterval` — `ParamType::U64 { min: 0, max: 3600 }`
- `ChainSyncIdleTimeout` — `ParamType::U64 { min: 0, max: 86400 }`

**Connection Limits section (new):**
- `AcceptedConnectionsLimit` — compound type in config JSON (`{"hardLimit": 512, "softLimit": 384, "delay": 5}`). In the schema, represent as a single `ParamType::String` with description noting the JSON object format, since `dugite-config` doesn't support compound editing. The default JSON representation should be included in `default_config_for_network()`.

**Genesis section (new):**
- All `SyncTargetNumberOf*` fields — `ParamType::U64` with same bounds as their deadline counterparts. **Correct defaults:** active=5, established=10, known=150, root=0, activeBLP=30, establishedBLP=40, knownBLP=100
- `MinBigLedgerPeersForTrustedState` — `ParamType::U64 { min: 0, max: 100 }`

Update `default_config_for_network()` to include all new fields with Haskell-matching defaults.

### Acceptance Criteria

- Every field added to `NodeConfig` has a corresponding `ParamDef` in `KNOWN_PARAMS`
- All new fields appear in the correct section in the TUI
- `default_config_for_network()` includes all new fields
- `cargo nextest run -p dugite-config` passes
- Existing config files with new fields parse correctly in the editor

---

## Implementation Order

1. **§1 — Governor local root promotion gaps** (exclude topology from aggregate paths, add `inProgressPromoteCold` tracking)
2. **§2 — Demotion exclusion audit + `aboveTargetLocal`** (audit existing exclusions, add hot→warm exception for oversubscribed groups)
3. **§3 — Config fields** (additive, no behaviour change)
4. **§6 — Startup validation** (depends on §3 for new fields)
5. **§7 — Schema alignment** (depends on §3 for field names)
6. **§4 — `diffusionMode` wiring** (one callsite change in connection_lifecycle.rs)
7. **§5 — Additional topology fields** (advertise integration, parse-only for rest)

---

## Testing Strategy

- **Unit tests** for each governor change: mock `PeerManager` state, verify emitted actions
- **Unit tests** for `aboveTargetLocal`: group with hot > hotValency → excess demoted
- **Unit tests** for config parsing: valid/invalid JSON, default values, edge cases
- **Unit tests** for startup validation: all 14 constraint predicates, both valid and invalid
- **Unit tests** for `AcceptedConnectionsLimit`: verify short key names (`hardLimit`, `softLimit`, `delay`)
- **Integration**: verify config compatibility with cardano-node 10.x JSON configs
- **Soak test**: run node on preview testnet, verify local root reconnection under peer churn

---

## Errata vs Issue #369

The following corrections were identified during spec review against Haskell source:

1. **Sync target defaults (issue §3):** Issue states all non-BLP sync targets are 0. Haskell `defaultSyncTargets` has: active=5, established=10, known=150, root=0. Only root is 0.
2. **SyncTargetNumberOfEstablishedBigLedgerPeers (issue §3):** Issue states default 50. Haskell default is 40.
3. **AcceptedConnectionsLimit JSON keys (issue §3):** Issue implies full camelCase field names. Haskell `FromJSON` (OrphanInstances.hs) uses short keys: `hardLimit`, `softLimit`, `delay`.
4. **ChainSyncIdleTimeout (issue §3):** Issue states "NoTimeout for GenesisMode, Timeout 300s for PraosMode". Haskell default is 3373s for ALL modes when no override set. 0 means no timeout; specific values override.
5. **EgressPollInterval (issue §3):** Issue states default 10s. Haskell `defaultEgressPollInterval` is 0.
6. **Per-group local root promotion (issue §1):** Issue implies this is entirely missing. `governor.rs` already has per-group promotion (Stage 1, lines 152-233). The gap is: aggregate paths don't exclude topology peers, and no `inProgressPromoteCold` tracking.
7. **Demotion exclusions (issue §2):** Issue implies only cold churn has topology exclusion. `select_worst_hot()`, `select_worst_warm()` already exclude topology peers.
8. **Haskell promotion strategy:** Haskell uses uniform random selection for promotions (`simplePromotionPolicy`), not score-based. Our score-based approach is a deliberate divergence.
