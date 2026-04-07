# Design: P2P Governor and Config Gaps vs Haskell cardano-node 10.x

**Issue:** #369  
**Date:** 2026-04-07  
**Status:** Draft

## Overview

Close the gaps between Dugite's P2P governor, topology handling, and config parsing compared to Haskell cardano-node 10.x. The Haskell implementation uses per-group local root peer management with unconditional demotion exclusions — our current governor uses aggregate peer counts and is source-blind in most demotion paths. This spec covers 7 work items ordered by priority.

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

Same pattern but checks `|membersActive| < hotValency` per group. An additional constraint: peers must pass `readyPeers` — they must have been warm for at least `policyPeerShareActivationDelay` before being eligible for hot promotion.

**Key properties:**
- Fires even when aggregate `targetNumberOfEstablishedPeers` is satisfied
- `belowTargetOther` explicitly excludes `LocalRootPeers.keysSet` from its candidate set — local roots are ONLY promoted by `belowTargetLocal`
- Guard ordering: `belowTargetBigLedgerPeers <> belowTargetLocal <> belowTargetOther` — local has priority via STM `<|>` (orElse) semantics

### Current Gap

`compute_actions()` in `governor.rs` uses aggregate counts (`warm + hot < target_warm`). No per-group iteration. Topology peers can be promoted by the same path as ledger/DNS peers — there is no independent local root guard.

### Design

Add a `LocalRootGroup` tracking struct to `PeerManager`:

```rust
pub struct LocalRootGroupState {
    pub name: String,
    pub members: HashSet<SocketAddr>,
    pub warm_valency: usize,
    pub hot_valency: usize,
}
```

`NodePeerManager` already stores `local_root_groups: Vec<LocalRootGroupInfo>` with `addrs`, `hot_valency`, `warm_valency`. Expose a method to query per-group established/active counts:

```rust
impl PeerManager {
    /// Returns groups where |members ∩ established| < warm_valency
    pub fn local_groups_below_warm_target(&self) -> Vec<(usize, Vec<SocketAddr>)>;
    
    /// Returns groups where |members ∩ active| < hot_valency  
    pub fn local_groups_below_hot_target(&self) -> Vec<(usize, Vec<SocketAddr>)>;
}
```

In `Governor::compute_actions()`, insert the local root check **before** aggregate target logic:

```
1. below_target_local_warm() → PromoteToWarm for deficient groups (from group members only)
2. below_target_local_hot()  → PromoteToHot for deficient groups (from group members only)
3. below_target_other_warm() → PromoteToWarm from non-local-root cold peers (existing logic)
4. below_target_other_hot()  → PromoteToHot from non-local-root warm peers (existing logic)
5. above_target_other_hot()  → DemoteToWarm (existing logic, with exclusion from §2)
6. Churn, discovery, etc.
```

The `belowTargetOther` paths must exclude `PeerSource::Topology` peers from their candidate sets — local roots should only be managed by the local root guards.

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

### Current Gap

Our code only excludes topology peers from `select_lowest_reputation_cold()` (cold churn eviction). The following paths are **source-blind**:
- `select_worst_hot()` — used for hot→warm when `hot > target_hot` (line ~153)
- Hot churn at line ~165 — calls `select_worst_hot()` 
- `select_worst_warm()` — used for warm→cold when `warm > target` (line ~193)
- `peer_failed()` forgets non-topology peers after 5 failures — correct

### Design

Add topology exclusion to every demotion/selection function in `selection.rs`:

1. **`select_worst_hot(peer_manager) -> Option<SocketAddr>`**: Add `if info.source == PeerSource::Topology { continue; }` filter
2. **`select_worst_warm(peer_manager) -> Option<SocketAddr>`**: Same exclusion
3. **New: `select_excess_local_hot(peer_manager, groups) -> Vec<SocketAddr>`**: For each local root group where `|hot_members| > hot_valency`, select `excess` worst-scoring hot members for demotion. This is the one exception path.

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
| `SyncTargetNumberOfActivePeers` | `usize` | `0` | Active peers during Genesis sync |
| `SyncTargetNumberOfEstablishedPeers` | `usize` | `0` | Established peers during sync |
| `SyncTargetNumberOfKnownPeers` | `usize` | `0` | Known peers during sync |
| `SyncTargetNumberOfRootPeers` | `usize` | `0` | Root peers during sync |
| `SyncTargetNumberOfActiveBigLedgerPeers` | `usize` | `30` | Active BLPs during sync |
| `SyncTargetNumberOfEstablishedBigLedgerPeers` | `usize` | `50` | Established BLPs during sync |
| `SyncTargetNumberOfKnownBigLedgerPeers` | `usize` | `100` | Known BLPs during sync |
| `MinBigLedgerPeersForTrustedState` | `usize` | `5` | Pause sync if active BLPs drop below this |

**Haskell `ConsensusMode` type:** Parsed from JSON string literals `"PraosMode"` or `"GenesisMode"` via derived `FromJSON`. Default is `PraosMode`. In `GenesisMode`, the governor switches between "deadline targets" (normal) and "sync targets" (when `LedgerStateJudgement = TooOld`). When `PraosMode`, sync targets are ignored.

#### Connection Management

| JSON Key | Type | Default | Purpose |
|----------|------|---------|---------|
| `AcceptedConnectionsLimit` | struct | `{ hard: 512, soft: 384, delay: 5.0 }` | Inbound connection limits |
| `ProtocolIdleTimeout` | `f64` (seconds) | `5.0` | Idle mini-protocol pruning |
| `TimeWaitTimeout` | `f64` (seconds) | `60.0` | Connection TIME_WAIT duration |
| `EgressPollInterval` | `f64` (seconds) | `0.0` | Outbound governor poll interval |
| `ChainSyncIdleTimeout` | `Option<f64>` (seconds) | `None` (means use default 3373s) | ChainSync-specific idle timeout |

**`AcceptedConnectionsLimit` JSON format:** Parsed as an object with fields `acceptedConnectionsHardLimit` (u32), `acceptedConnectionsSoftLimit` (u32), `acceptedConnectionsDelay` (f64 seconds). The delay is NOT fixed — it scales linearly from 0 at soft limit to `delay` at hard limit. Above hard limit: block until count drops below, then wait `delay`.

### Design

Add a `ConsensusMode` enum and `AcceptedConnectionsLimit` struct to `config.rs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsensusMode {
    PraosMode,
    GenesisMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedConnectionsLimit {
    pub accepted_connections_hard_limit: u32,
    pub accepted_connections_soft_limit: u32,
    pub accepted_connections_delay: f64,
}
```

Add all fields to `NodeConfig` with `#[serde(default)]` using Haskell-matching defaults. **Phase 1** (this issue): parse and store. **Phase 2** (future): wire `AcceptedConnectionsLimit` to the N2N listener, Genesis targets to the governor.

### Acceptance Criteria

- All fields parse from cardano-node config JSON without errors
- Missing fields use Haskell-matching defaults
- `ConsensusMode` accepts exactly `"PraosMode"` and `"GenesisMode"`
- `AcceptedConnectionsLimit` parses as nested object with camelCase field names
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

### Current Gap

`topology.rs` already parses `diffusion_mode: Option<String>` on `LocalRootGroup`. But this value is **not wired** to the handshake. All outbound connections use the global `DiffusionMode` from `NodeConfig`.

### Design

1. Store the parsed `diffusion_mode` in `LocalRootGroupInfo` (in `networking.rs`)
2. Add a method `peer_diffusion_mode(&self, addr: &SocketAddr) -> DiffusionMode` to `NodePeerManager` that:
   - Looks up which local root group the peer belongs to
   - Returns the group's `diffusion_mode` if set, otherwise the global `DiffusionMode`
3. In `connection_lifecycle.rs`, when establishing an outbound connection to a topology peer, call `peer_diffusion_mode()` to determine the handshake `initiator_only` flag instead of using the global config

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

**Current state:** Already parsed in `LocalRootGroup`. Verify it's wired to peer sharing responses.

**Design:** Audit the peer sharing response path to ensure peers from `advertise: false` groups are excluded.

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
- `AcceptedConnectionsLimit` — compound type; represent as three separate keys:
  - `AcceptedConnectionsHardLimit` — `ParamType::U64 { min: 1, max: 65535 }`
  - `AcceptedConnectionsSoftLimit` — `ParamType::U64 { min: 1, max: 65535 }`
  - `AcceptedConnectionsDelay` — `ParamType::U64 { min: 0, max: 60 }`

**Genesis section (new):**
- All `SyncTargetNumberOf*` fields — `ParamType::U64` with same bounds as their deadline counterparts
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

1. **§2 — Demotion exclusion** (smallest change, highest correctness impact)
2. **§1 — `below_target_local`** (depends on group tracking, most complex governor change)
3. **§3 — Config fields** (additive, no behaviour change)
4. **§6 — Startup validation** (depends on §3 for new fields)
5. **§7 — Schema alignment** (depends on §3 for field names)
6. **§4 — `diffusionMode` wiring** (independent but medium complexity)
7. **§5 — Additional topology fields** (lowest priority, mostly verification)

---

## Testing Strategy

- **Unit tests** for each governor change: mock `PeerManager` state, verify emitted actions
- **Unit tests** for config parsing: valid/invalid JSON, default values, edge cases
- **Unit tests** for startup validation: all constraint violations produce clear errors
- **Integration**: verify config compatibility with cardano-node 10.x JSON configs
- **Soak test**: run node on preview testnet, verify local root reconnection under peer churn
