# Design: P2P Governor and Config Gaps vs Haskell cardano-node 10.x

**Issue:** #369  
**Date:** 2026-04-07  
**Status:** Draft

## Overview

Close the remaining gaps between Dugite's P2P governor, topology handling, and config parsing compared to Haskell cardano-node 10.x. Our governor already has per-group local root promotion (`governor.rs` lines 152-233) and topology demotion exclusions in selection functions (`select_worst_hot`, `select_worst_warm`, `select_lowest_reputation_cold`). All config fields already exist but several have wrong defaults or types. The remaining work is: ensuring aggregate target paths exclude topology+BLP peers, adding the `aboveTargetLocal` exception path, adding in-progress promotion tracking, fixing config defaults/types to match Haskell exactly, completing startup validation, wiring per-group `diffusionMode` to handshakes, and integrating the `advertise` flag in peer sharing. This spec covers 7 work items ordered by priority.

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

1. **Exclude topology peers from aggregate paths:** In Stage 2 (`below_target_other_warm`), filter out `PeerSource::Topology` from the eligible set. In Stage 3 (`below_target_other_hot`), same exclusion. This matches Haskell's `belowTargetOther` which explicitly excludes `LocalRootPeers.keysSet`.

2. **Exclude big ledger peers from aggregate paths:** Haskell's `belowTargetOther` in `ActivePeers.hs` (warm→hot) explicitly excludes `bigLedgerPeersSet` — BLPs are promoted only by their own dedicated `belowTargetBigLedgerPeers` path. Our Stage 3 should also exclude BLPs. For Stage 2 (cold→warm), Haskell excludes BLPs indirectly (they have their own cold→warm path); verify our code doesn't double-promote BLPs.

3. **Add in-progress tracking sets to `Governor`:** Haskell's `PeerSelectionState` tracks 5 in-progress sets (all flat global `Set peeraddr`):
   - `inProgressPromoteCold: HashSet<SocketAddr>` — cold→warm in-flight
   - `inProgressPromoteWarm: HashSet<SocketAddr>` — warm→hot in-flight
   - `inProgressDemoteWarm: HashSet<SocketAddr>` — hot→warm in-flight (used in `aboveTargetLocal`)
   - `inProgressDemoteHot: HashSet<SocketAddr>` — hot→warm in-flight (separate tracking)
   - `inProgressDemoteToCold: HashSet<SocketAddr>` — any→cold async teardown
   
   Our `already_promoted` is per-invocation only (governor.rs line 158). For correctness, at minimum add `in_progress_promote_cold` (prevents double cold→warm across ticks) and `in_progress_promote_warm` (prevents double warm→hot). Populated when actions are emitted, cleared on completion callback. Subtract from candidate sets in both local and aggregate paths. The `numToPromote` per-group calculation should subtract `|inProgressPromoteCold ∩ group.members|`.

4. **Verify backoff filtering:** Ensure Stage 1 correctly intersects group members with `peers_eligible_to_connect()` results (peers whose `next_connect_after` has elapsed or is None). The code review confirmed this is already done (line 193).

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

2. **New: `above_target_local_hot(peer_manager, groups) -> Vec<GovernorAction>`**: For each local root group where `|hot_members| > hot_valency`:
   - Compute `availableToDemote = (localRootPeers ∩ activePeers) \ inProgressDemoteHot \ inProgressDemoteToCold`
   - Per group: `membersAvailableToDemote = group.members ∩ availableToDemote`
   - `numToDemote = |membersActive| - hotValency - |inProgressDemoteHot ∩ group.members|`
   - Select `numToDemote` worst-scoring hot members from that group for `DemoteToWarm`
   - Haskell uses `policyPickHotPeersToDemote` (ChainSync + BlockFetch metrics). Our score-based approach is aligned in principle — use `peer_score()` to select lowest-scoring peers.

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

## 3. P2P Config Default Corrections

**Priority:** High (wrong defaults affect real node behaviour)

### Current State

All config fields referenced in issue #369 **already exist** in `config.rs`. The structs (`ConsensusMode`, `AcceptedConnectionsLimit`), the sync target fields, and the connection management fields are all present and parse correctly. However, several defaults are **wrong** compared to Haskell.

### Bugs: Wrong Defaults

#### Deadline target defaults (lines 629-632)

Haskell's `defaultDeadlineTargets` varies by `BlockProducerOrRelay`:

| Field | Our default | Haskell Relay | Haskell BP | Fix |
|-------|-------------|---------------|------------|-----|
| `target_number_of_root_peers` | 60 | 60 | 100 | Correct for relay; BP-awareness is future work |
| `target_number_of_known_peers` | **85** | **150** | 100 | **Fix to 150** |
| `target_number_of_established_peers` | **40** | **30** | 30 | **Fix to 30** |
| `target_number_of_active_peers` | **15** | **20** | 20 | **Fix to 20** |
| BLP targets | Correct | 15/10/5 | 15/10/5 | No change |

> **Source:** `defaultDeadlineTargets` in `ouroboros-network/lib/Ouroboros/Network/Diffusion/Configuration.hs`. Haskell checks `if hasProtocolFile protocolFiles then BlockProducer else Relay`. For now, use Relay defaults; BP-specific defaults are future work (requires detecting operational certificate presence).

#### Sync target defaults (lines 646-649)

| Field | Our default | Haskell | Fix |
|-------|-------------|---------|-----|
| `sync_target_number_of_active_peers` | **0** | **5** | **Fix to 5** |
| `sync_target_number_of_established_peers` | **0** | **10** | **Fix to 10** |
| `sync_target_number_of_known_peers` | **0** | **150** | **Fix to 150** |
| `sync_target_number_of_root_peers` | 0 | 0 | Correct |
| `sync_target_number_of_active_big_ledger_peers` | 30 | 30 | Correct |
| `sync_target_number_of_established_big_ledger_peers` | **50** | **40** | **Fix to 40** |
| `sync_target_number_of_known_big_ledger_peers` | 100 | 100 | Correct |
| `min_big_ledger_peers_for_trusted_state` | 5 | 5 | Correct |

> **Critical:** Our current sync defaults of `active=0, established=0, known=0` would **fail** `sanePeerSelectionTargets` validation (since `active <= established <= known` requires them all to be 0 or all properly ordered, but the BLP targets are non-zero and fine). Actually 0 <= 0 <= 0 passes the chain, but these zeros mean Genesis mode would be non-functional. Match Haskell.

#### Connection management defaults (lines 392-393)

| Field | Our default | Haskell | Fix |
|-------|-------------|---------|-----|
| `egress_poll_interval` | **10** | **0** | **Fix to 0** |
| `protocol_idle_timeout` | 5 | 5 | Correct |
| `time_wait_timeout` | 60 | 60 | Correct |

#### Timeout field types (lines 270-276)

| Field | Our type | Haskell type | Fix |
|-------|----------|-------------|-----|
| `protocol_idle_timeout` | **`u64`** | `DiffTime` (fractional) | **Change to `f64`** |
| `time_wait_timeout` | **`u64`** | `DiffTime` (fractional) | **Change to `f64`** |
| `egress_poll_interval` | **`u64`** | `DiffTime` (fractional) | **Change to `f64`** |
| `chain_sync_idle_timeout` | `Option<u64>` | `DiffTime` (fractional) | **Change to `Option<f64>`** |

Haskell's `DiffTime` is picosecond-precision `Rational`. Aeson's `FromJSON DiffTime` accepts both integer and fractional JSON numbers (`5`, `5.5`, `0.1`). Our `u64` would reject valid fractional values from real cardano-node configs.

### Design

Fix all defaults and types in `config.rs`:

1. Change deadline target defaults to match Haskell Relay: `known=150, established=30, active=20`
2. Change sync target defaults: `active=5, established=10, known=150, established_blp=40`
3. Change `egress_poll_interval` default to `0`
4. Change timeout types from `u64` to `f64`
5. Update all `default_*` helper functions accordingly
6. Update unit tests to reflect corrected defaults

### Acceptance Criteria

- All defaults match Haskell `defaultDeadlineTargets(Relay)` and `defaultSyncTargets`
- Timeout fields accept fractional values (e.g., `"ProtocolIdleTimeout": 5.5`)
- `EgressPollInterval` defaults to 0
- Existing unit tests updated and passing
- New unit test: parse config with fractional timeout values

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

**Priority:** High (existing validation is incomplete)

### Current State

`config.rs` already has a `validate()` method (line 502) that checks:
- `known >= established` and `established >= active` for regular peers
- Same ordering for BLP targets
- Sync targets checked **only when `ConsensusMode::GenesisMode`**

### Bugs vs Haskell

1. **Sync targets validated conditionally.** Haskell validates BOTH deadline AND sync targets **unconditionally** — the check is `unless (sanePeerSelectionTargets deadlineTargets && sanePeerSelectionTargets syncTargets)` with no mode guard. Our code wraps sync validation in `if self.consensus_mode == ConsensusMode::GenesisMode`.

2. **Missing constraints.** Haskell's `sanePeerSelectionTargets` has 14 predicates. Our validation is missing:
   - `root <= known` (regular peers)
   - `active <= 100` (regular peers)
   - `established <= 1000` (regular peers)
   - `known <= 10000` (regular peers)
   - `activeBLP <= 100`
   - `establishedBLP <= 1000`
   - `knownBLP <= 10000`
   - All of the above for sync targets
   - Sync target BLP ordering (`activeBLP <= establishedBLP <= knownBLP`)

3. **Missing sync BLP validation entirely.** Even when GenesisMode is set, our code only validates sync regular targets, not sync BLP targets.

### Haskell Reference

`sanePeerSelectionTargets` in `Governor/Types.hs`:

```haskell
sanePeerSelectionTargets PeerSelectionTargets{..} =
                                 0 <= targetNumberOfActivePeers
 && targetNumberOfActivePeers      <= targetNumberOfEstablishedPeers
 && targetNumberOfEstablishedPeers <= targetNumberOfKnownPeers
 &&      targetNumberOfRootPeers   <= targetNumberOfKnownPeers
 &&                              0 <= targetNumberOfRootPeers
 &&                                       0 <= targetNumberOfActiveBigLedgerPeers
 && targetNumberOfActiveBigLedgerPeers      <= targetNumberOfEstablishedBigLedgerPeers
 && targetNumberOfEstablishedBigLedgerPeers <= targetNumberOfKnownBigLedgerPeers
 && targetNumberOfActivePeers      <= 100
 && targetNumberOfEstablishedPeers <= 1000
 && targetNumberOfKnownPeers       <= 10000
 && targetNumberOfActiveBigLedgerPeers      <= 100
 && targetNumberOfEstablishedBigLedgerPeers <= 1000
 && targetNumberOfKnownBigLedgerPeers       <= 10000
```

Called from `makeNodeConfiguration` unconditionally for both deadline and sync targets.

### Design

Replace the existing validation with a complete `sane_peer_selection_targets()` function matching all 14 Haskell predicates. Apply unconditionally to both deadline and sync target sets.

```rust
fn sane_peer_selection_targets(
    label: &str,
    active: usize, established: usize, known: usize, root: usize,
    active_blp: usize, established_blp: usize, known_blp: usize,
) -> Result<()> {
    // 14 predicates matching Haskell exactly:
    // 1.  0 <= active                          (always true for usize)
    // 2.  active <= established
    // 3.  established <= known
    // 4.  root <= known
    // 5.  0 <= root                            (always true for usize)
    // 6.  0 <= active_blp                      (always true for usize)
    // 7.  active_blp <= established_blp
    // 8.  established_blp <= known_blp
    // 9.  active <= 100
    // 10. established <= 1000
    // 11. known <= 10000
    // 12. active_blp <= 100
    // 13. established_blp <= 1000
    // 14. known_blp <= 10000
}
```

Call unconditionally:
```rust
sane_peer_selection_targets("deadline", ...deadline fields...)?;
sane_peer_selection_targets("sync", ...sync fields...)?;
```

### Acceptance Criteria

- Validation covers all 14 predicates for both deadline and sync targets
- Sync targets validated **unconditionally** (not just in GenesisMode)
- Upper bounds enforced: active ≤ 100, established ≤ 1000, known ≤ 10000 (both regular and BLP)
- Root peers validated: `root ≤ known`
- Default config values (after §3 fixes) pass validation
- Unit tests for each individual constraint violation

---

## 7. dugite-config: Schema Alignment

**Priority:** Medium

### Current State

Need to verify whether `schema.rs` already has `ParamDef` entries for the Genesis/connection management fields, and whether existing deadline target defaults match the corrected Haskell values.

### Design

1. **Fix existing deadline target defaults** in `default_config_for_network()`: `known=150, established=30, active=20` (matching §3 corrections)
2. **Verify/add `ParamDef` entries** for all Genesis and connection management fields:

**Network section (existing):**
- `ConsensusMode` — `ParamType::Enum { values: &["PraosMode", "GenesisMode"] }`
- `ProtocolIdleTimeout` — `ParamType::U64 { min: 0, max: 3600 }` (seconds, note: config.rs accepts f64 but schema UI shows integer)
- `TimeWaitTimeout` — `ParamType::U64 { min: 0, max: 3600 }`
- `EgressPollInterval` — `ParamType::U64 { min: 0, max: 3600 }`, default **0** (not 10)
- `ChainSyncIdleTimeout` — `ParamType::U64 { min: 0, max: 86400 }`, default empty (3373s implicit)

**Connection Limits section (new):**
- `AcceptedConnectionsLimit` — compound type; represent in `default_config_for_network()` as nested JSON object with short keys (`hardLimit`, `softLimit`, `delay`)

**Genesis section:**
- All `SyncTargetNumberOf*` fields with corrected defaults: active=5, established=10, known=150, root=0, activeBLP=30, establishedBLP=40, knownBLP=100
- `MinBigLedgerPeersForTrustedState` — default 5

### Acceptance Criteria

- Every field added to `NodeConfig` has a corresponding `ParamDef` in `KNOWN_PARAMS`
- All new fields appear in the correct section in the TUI
- `default_config_for_network()` includes all new fields
- `cargo nextest run -p dugite-config` passes
- Existing config files with new fields parse correctly in the editor

---

## Implementation Order

1. **§3 — Config default/type corrections** (fix wrong defaults, change timeout types to f64 — foundational, affects validation)
2. **§6 — Startup validation** (complete the 14-predicate check, make unconditional — depends on §3 for correct defaults)
3. **§1 — Governor local root promotion gaps** (exclude topology+BLP from aggregate paths, add `inProgressPromoteCold` tracking)
4. **§2 — Demotion exclusion audit + `aboveTargetLocal`** (confirm existing exclusions, add hot→warm exception for oversubscribed groups)
5. **§4 — `diffusionMode` wiring** (one callsite change in connection_lifecycle.rs)
6. **§7 — Schema alignment** (update defaults in schema to match §3 corrections)
7. **§5 — Additional topology fields** (advertise integration in peer sharing, parse-only for rest)

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

### Additional findings from deep review (not in issue #369)

9. **Deadline target defaults wrong in existing code:** Our defaults `known=85, established=40, active=15` don't match Haskell Relay defaults `known=150, established=30, active=20`. Source: `defaultDeadlineTargets(Relay)` in `ouroboros-network/Diffusion/Configuration.hs`.
10. **Sync target defaults wrong in existing code:** Our sync defaults `active=0, established=0, known=0, established_blp=50` don't match Haskell `active=5, established=10, known=150, established_blp=40`.
11. **EgressPollInterval default wrong in existing code:** Our default is 10, Haskell is 0.
12. **Timeout types wrong:** Our `protocol_idle_timeout`, `time_wait_timeout`, `egress_poll_interval`, `chain_sync_idle_timeout` use `u64`/`Option<u64>`. Haskell uses `DiffTime` which accepts fractional seconds. Should be `f64`/`Option<f64>`.
13. **Validation is conditional:** Our sync target validation is gated by `consensus_mode == GenesisMode`. Haskell validates BOTH sets unconditionally.
14. **Validation is incomplete:** Missing `root <= known`, upper bounds (`active <= 100`, `established <= 1000`, `known <= 10000`), BLP upper bounds, and sync BLP ordering checks.
15. **Big ledger peers not excluded from belowTargetOther:** Haskell's `belowTargetOther` in `ActivePeers.hs` explicitly excludes `bigLedgerPeersSet` from warm→hot promotion candidates. Our aggregate Stage 3 does not.
16. **BP vs Relay different defaults:** Haskell's `defaultDeadlineTargets` varies by `BlockProducerOrRelay` — BP gets `root=100, known=100`, Relay gets `root=60, known=150`. Our code uses a single default set. Detecting BP mode requires checking for operational certificate presence (`hasProtocolFile`).
17. **Haskell aboveTargetLocal demotion is metric-based:** Uses `policyPickHotPeersToDemote` — ranks by ChainSync tips + BlockFetch completions, demotes lowest-utility peers. Our score-based approach for demotions is conceptually aligned.
18. **Churn localRootHotTarget = SUM of group hotValencies:** Not max. Important for the churn floor calculation `max 1 (sum of all hotValencies)`.
19. **Peer sharing advertise filtering not integrated:** `is_advertisable()` exists in NodePeerManager but is not called from peer sharing response path. Only `is_routable()` filtering is applied.
20. **Config fields already exist:** Issue §3 says "Fields to Add" but `ConsensusMode`, `AcceptedConnectionsLimit`, all sync targets, and connection management fields are already present in `config.rs`. The work is fixing defaults, types, and validation — not adding fields.
