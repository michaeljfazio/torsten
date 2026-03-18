# P2P Governor: Upgrade to Ouroboros Peer Selection

This document describes the architecture for upgrading Torsten's peer management
to a proper implementation of the Haskell cardano-node Ouroboros P2P governor
(issue #138).

---

## 1. Current State

### What Exists Today

Two modules implement peer management in `torsten-network`:

**`peer_manager.rs`** — The data layer. Tracks every known peer in a flat
`HashMap<SocketAddr, PeerInfo>` together with three `HashSet`s for the
cold/warm/hot buckets. Key features implemented:

| Feature | Status |
|---|---|
| Cold / Warm / Hot temperature tracking | Done |
| `PeerCategory` (LocalRoot, PublicRoot, BigLedgerPeer, LedgerPeer, Shared, Bootstrap) | Done |
| `ConnectionDirection` (Inbound / Outbound) | Done |
| `PeerSource` (Config, PeerSharing, Ledger) | Done |
| `PeerPerformance` — EWMA handshake RTT + block fetch latency | Done |
| Reputation scoring (latency + volume + reliability + recency) | Done |
| Circuit breaker (Closed / Open / HalfOpen with exponential cooldown) | Done |
| Subnet diversity penalty for peer selection (/24 IPv4, /48 IPv6) | Done |
| Trustable-first two-tier ordering for `peers_to_connect()` | Done |
| Inbound connection count limit | Done |
| `DiffusionMode` (InitiatorOnly / InitiatorAndResponder) | Done |
| Failure-count time decay (halves every 5 min) | Done |

**`governor.rs`** — The policy layer. Runs on a 30-second `tokio::interval` in
`torsten-node`. Features implemented:

| Feature | Status |
|---|---|
| `PeerTargets` (root/known/established/active + BLP variants) | Done |
| Sync-state-aware target switching (`PreSyncing` / `Syncing` / `CaughtUp`) | Done |
| Hard/soft connection limits with `ConnectionDecision` | Done |
| Big-ledger-peer promotion priority | Done |
| Active (hot) peer target enforcement | Done |
| Established (warm+hot) target enforcement | Done |
| Surplus reduction — demote/disconnect by lowest reputation, local-root protected | Done |
| Churn mechanism — 20% target reduction cycle at configurable intervals | Done |
| Default targets matching cardano-node (active=15, established=40, known=85) | Done |

### How It Is Wired

The governor runs as a standalone `tokio::spawn` task in `node/mod.rs`.
Every 30 seconds it:
1. Acquires a read lock on `Arc<RwLock<PeerManager>>` and calls `governor.evaluate()` and `governor.maybe_churn()`.
2. Acquires a write lock and applies the resulting `GovernorEvent`s by calling
   `promote_to_hot`, `demote_to_warm`, `peer_disconnected`, and
   `recompute_reputations`.
3. `GovernorEvent::Connect` is acknowledged but not executed here — outbound
   connections originate from the main connection loop via `peers_to_connect()`.

### Honest Assessment of the Gaps

The current implementation is a **functional scaffold**, not a production-grade
Ouroboros peer selection governor. It passes all existing tests and correctly
manages peer temperatures, but it diverges from the Haskell specification in
several important ways that affect network health under real conditions:

---

## 2. Target State: The Haskell Ouroboros Governor

The reference implementation is `ouroboros-network/ouroboros-network/src/Ouroboros/Network/PeerSelection/Governor/`
in the Haskell cardano-node repository. The key behavioral contracts are:

### 2.1 Peer Selection State Machine

The Haskell governor drives peers through a formal state machine with
four named states:

```
Known (cold)
  │  promote-to-warm (TCP connect + handshake)
  ▼
Established (warm) ◄──── demote-to-cold (disconnect)
  │  promote-to-hot  (activate mini-protocols)
  ▼
Active (hot)        ◄──── demote-to-warm (deactivate mini-protocols)
  │
  ▼ demote-to-cold (forceful disconnect)
Known
```

Each transition has an associated **timeout** and **backoff** policy so that
a stuck or slow peer is eventually abandoned rather than blocking the state
machine. The governor never blocks waiting for a single peer — all transitions
are concurrent and driven by a single select loop that waits for the earliest
of: a timer expiry, a target count change, a peer status update, or a churn
event.

### 2.2 Target Counts

The governor maintains **six independent target counts**:

| Variable | Haskell Name | Default |
|---|---|---|
| Known peers | `TargetNumberOfKnownPeers` | 100 |
| Established peers | `TargetNumberOfEstablishedPeers` | 40 |
| Active peers | `TargetNumberOfActivePeers` | 15 |
| Known big-ledger peers | `TargetNumberOfKnownBigLedgerPeers` | 15 |
| Established big-ledger peers | `TargetNumberOfEstablishedBigLedgerPeers` | 10 |
| Active big-ledger peers | `TargetNumberOfActiveBigLedgerPeers` | 5 |

When any target is not met, the governor immediately attempts to satisfy the
deficit. When any target is exceeded, the governor immediately demotes the
lowest-priority surplus peers. The decision loop runs continuously, sleeping
only when all targets are satisfied.

### 2.3 Local Root Peer Pinning

Local root peers (from `localRoots` in the topology file) have **pinned
targets** that override the normal target counts. The governor treats local
roots as a separate mandatory bucket: if we have fewer connected local-root
peers than configured, those deficit connections are pursued before any
non-root target. Local roots are never demoted for surplus reduction and are
never churned.

The topology `localRoots` entry carries a `valency` field which is the minimum
number of peers from that root group that must remain connected at all times.
This is the principal difference from `publicRoots` (which are just cold seeds
with no connection guarantee).

### 2.4 Churn

The Haskell governor performs two types of churn:

**Deadline churn (normal mode)** — Every ~55 minutes, a fraction of established
and active peers are opportunistically replaced. The governor reduces the known
peer target by the churn amount (causing cold evictions), then resets the
target (causing connects to new peers). This rotates approximately 1-2 peers
per cycle.

**Bulk sync churn** — During active block download, churn cycles are more
aggressive (~15 minutes) to allow the governor to quickly shed peers with
poor block-fetch performance and try alternatives.

Key properties of churn that are NOT present in the current implementation:
- Churn must respect local root protection (local roots never churned).
- Churn must track the actual replaced set so that previously-churned
  peers are not immediately re-selected.
- Churn randomises which peers are replaced (not deterministically
  worst-reputation-first) to avoid repeatedly cycling through the same
  set of mediocre peers.
- The Haskell implementation uses a separate churn governor loop that
  interleaves with the main selection loop via an STM variable; it does
  not modify the targets directly.

### 2.5 Peer Sharing Integration

When the peer table has fewer than `TargetNumberOfKnownPeers` cold peers, the
governor triggers peer-sharing requests to warm/hot peers that have the
`PeerSharingEnabled` flag set in their handshake. The governor tracks:
- How many peer-sharing requests are in-flight per peer.
- Whether a given peer has already been asked this churn cycle.
- Discovered peers go into a "gossip bucket" rather than being immediately
  used to satisfy established/active targets (they must cool down first).

The current implementation has the peer-sharing protocol wire format fully
implemented (`peersharing.rs`) and can make requests, but the governor does
not drive the request/response cycle — it happens opportunistically in the
client connection loop rather than being target-driven.

### 2.6 Big Ledger Peer Preference During Sync

Big ledger peers (SPOs in the top 90% of stake, obtained via
`GetLedgerPeerSnapshot`) serve as trusted anchors during bulk block download.
The governor maintains a separate target bucket for them. When `SyncState` is
`Syncing` or `PreSyncing`, the BLP targets take absolute priority: the governor
fills BLP targets before any non-BLP target. This is partially implemented but
the BLP connect deficit (finding and connecting cold BLPs) is not wired — the
governor emits a comment instead of a concrete `Connect` event for BLPs.

### 2.7 Demotion Timeouts and Backoff

The Haskell governor tracks a **demotion timeout** per peer: if a warm-to-hot
promotion does not complete within N seconds, the peer is demoted back to warm.
If a cold-to-warm connection does not complete within N seconds, the peer is
demoted back to cold and a backoff timer is started.

The current implementation relies entirely on the circuit breaker in
`PeerManager` for this behaviour, which is subtly different: the circuit
breaker fires on the number of consecutive TCP failures, not on protocol-level
promotion timeouts.

---

## 3. Gap Analysis

The table below maps each Haskell governor feature against the current
Torsten implementation:

| Feature | Current State | Gap Severity |
|---|---|---|
| Cold → Warm → Hot state machine | Implemented (temperature field) | Low — data model correct |
| Transition timeouts (stuck peer detection) | Missing | Medium — can hold resources |
| Local root valency enforcement | Partial — local roots not demoted, but valency per-group not tracked | Medium |
| Per-group valency for topology `localRoots` | Missing — only a flat category flag | High |
| Churn: local root exemption | Implemented (not demoted) | Low |
| Churn: randomised peer replacement | Missing — currently worst-reputation-first | Low-Medium |
| Churn: avoid re-selecting recently-churned peers | Missing | Low |
| Churn: separate churn loop from selection loop | Missing — single 30s polling loop | Medium |
| Peer-sharing: governor-driven requests when below known-target | Missing | Medium |
| Peer-sharing: gossip bucket cooldown before promotion | Missing | Low |
| BLP: governor emits Connect events for cold BLPs | Missing — connect not emitted | High |
| BLP: prefer BLPs during sync, not just promote warm ones | Partial | Medium |
| Known peer target enforcement (eviction of excess cold peers) | Partial — eviction exists but not target-driven | Medium |
| Established peer target: separate from active target | Implemented | Low |
| Demotion timeout tracking per peer | Missing | Medium |
| Governor-driven connect rate limiting | Missing — unlimited connects per cycle | Medium |
| Peer selection diversity: penalise same ASN | Missing (only /24 subnet) | Low |

The two highest-severity gaps are:
1. **BLP connect events not emitted** — `evaluate_blp()` in `governor.rs` has a
   comment saying "The node will handle connecting cold BLPs through the regular
   connect path" but this is not wired. Cold BLPs are never promoted unless they
   happen to be selected by the general `peers_to_connect()` path.
2. **Per-group local root valency** — The Haskell topology allows multiple
   `localRoots` groups each with their own `valency` (minimum connections).
   Torsten tracks only a boolean `is_local_root` flag per peer.

---

## 4. Implementation Plan

The upgrade is broken into five phases, ordered by impact and dependency. Each
phase is independently committable and tested. Later phases build on earlier
ones but do not require a single large refactor.

---

### Phase 1: Fix BLP Connect Path and Add Per-Group Valency

**Effort:** Small (2-3 days)
**Issue coverage:** #138 (partial), #156 (partial)

This is the most impactful single fix because it corrects a silent gap where
the node never proactively connects to big-ledger peers during sync.

**Changes required:**

`crates/torsten-network/src/governor.rs`
- Change `evaluate_blp()` to emit `GovernorEvent::Connect(addr)` for cold BLPs
  when the established-BLP count is below `targets.established_blp`.
- Add a `PeerManager::cold_blp_addrs()` query method to enumerate cold BLPs
  eligible for connection (not circuit-open, not already in-flight).

`crates/torsten-network/src/peer_manager.rs`
- Add `LocalRootGroup` struct:
  ```
  struct LocalRootGroup {
      peers: Vec<SocketAddr>,
      valency: usize,           // minimum connected count
      advertise: bool,
  }
  ```
- Add `local_root_groups: Vec<LocalRootGroup>` to `PeerManager`.
- Add `add_local_root_group(peers, valency, advertise)` replacing the current
  flat `add_config_peer(addr, trustable=true, ...)` calls for local roots.
- Add `local_root_deficit()` → `Vec<SocketAddr>`: returns addresses from
  local-root groups where connected count < valency.

`crates/torsten-network/src/governor.rs`
- Add `evaluate_local_root_deficit()` phase before BLP evaluation. If any
  local-root group is below valency, emit `Connect` events for those peers
  first, before any other policy.

`crates/torsten-node/src/config.rs`
- Parse `valency` field from `localRoots` topology entries (already in JSON,
  currently ignored).
- Call `add_local_root_group()` instead of individual `add_config_peer()`.

**Key files changed:**
- `crates/torsten-network/src/governor.rs`
- `crates/torsten-network/src/peer_manager.rs`
- `crates/torsten-node/src/config.rs`
- `crates/torsten-node/src/node/mod.rs` (call new API)

---

### Phase 2: Formalise the Promotion/Demotion State Machine with Timeouts

**Effort:** Medium (4-5 days)
**Issue coverage:** #138 (core)

Replace the implicit `PeerTemperature` field (mutated from multiple places) with
an explicit state machine that enforces transition timeouts. This prevents
resource leaks from half-open connections and stuck promotion attempts.

**New type:**

`crates/torsten-network/src/peer_manager.rs`

```
/// Ouroboros peer selection state, tracking in-progress transitions.
enum PeerSelectionState {
    /// Known but not connected.
    Cold,
    /// TCP connect in progress. Started at `started_at`.
    ConnectingToWarm { started_at: Instant },
    /// Connected (warm). Ready for promotion.
    Warm { connected_at: Instant },
    /// Hot promotion in progress (mini-protocols activating). Started at `started_at`.
    PromotingToHot { started_at: Instant },
    /// Fully active (hot).
    Hot { promoted_at: Instant },
    /// Demotion in progress (mini-protocols deactivating).
    DemotingToWarm { started_at: Instant },
    /// Cold demotion in progress (disconnecting).
    DemotingToCold { started_at: Instant },
}
```

The existing `PeerTemperature` (Cold/Warm/Hot) is a coarse projection of this
state and can be derived from it for backwards compatibility with the governor's
evaluation logic.

Transition timeouts:
- `ConnectingToWarm` → abandon after 10 seconds (TCP connect timeout)
- `PromotingToHot` → abandon after 30 seconds (protocol activation timeout)
- `DemotingToWarm` → force-complete after 15 seconds
- `DemotingToCold` → force-complete after 10 seconds

**New governor method:**

`governor.rs` — `check_transition_timeouts(pm)` → `Vec<GovernorEvent>`

Scans all peers in transitioning states, emits `Demote` or `Disconnect` for
peers that have exceeded their timeout. Called alongside `evaluate()` and
`maybe_churn()` in the node's governor task.

**Key files changed:**
- `crates/torsten-network/src/peer_manager.rs` (new state enum)
- `crates/torsten-network/src/governor.rs` (timeout check method)
- `crates/torsten-node/src/node/mod.rs` (call timeout check)
- `crates/torsten-network/src/client.rs` (report promotion success/failure)

---

### Phase 3: Improve Churn (Randomised, No Re-selection, Local Root Exempt)

**Effort:** Small-Medium (2-3 days)
**Issue coverage:** #138 (churn correctness)

The current churn implementation reduces all targets by 20% and then restores
them. This is structurally correct but has three defects described in section 3.

**Changes:**

`crates/torsten-network/src/governor.rs`

- Add `churned_this_cycle: HashSet<SocketAddr>` to `Governor`. Reset when churn
  phase completes.
- Change `evaluate_surplus()` to select surplus peers randomly rather than
  worst-reputation-first when `churn_active` is true. Use the peer's address
  bytes as an entropy source xored with a per-churn random seed stored in
  `Governor` (`churn_seed: u64`, set at churn start from `Instant::now().elapsed().subsec_nanos()`).
- Skip peers that are in `churned_this_cycle` when selecting new connections
  after churn, preventing the same peer from being immediately re-selected.
  Peers age out of the churn set when the next full churn cycle begins.
- Explicitly skip local-root peers in the surplus reduction path when
  `churn_active` is true (they are already skipped for `is_local_root()` in
  `evaluate_surplus`, but the check should be documented and tested explicitly
  for churn).

**Key files changed:**
- `crates/torsten-network/src/governor.rs`

---

### Phase 4: Governor-Driven Peer Sharing Requests

**Effort:** Medium (3-4 days)
**Issue coverage:** #138 (gossip-driven peer table growth)

Currently peer sharing requests are made ad-hoc by the connection loop. The
governor should drive them when the known-peer count is below target.

**New governor phase in `evaluate()`:**

`governor.rs` — `evaluate_peer_sharing(pm, targets)` → `Vec<GovernorEvent>`

When `pm.cold_peer_count() + pm.warm_peer_count() + pm.hot_peer_count() < targets.known_peers`:
- Select up to `PEER_SHARING_REQUEST_COUNT` (default 2) warm/hot peers that
  have `peer_sharing_enabled = true` and have not been asked within the last
  churn interval.
- Emit a new `GovernorEvent::RequestPeerSharing(SocketAddr, amount: u8)`.

**New `GovernorEvent` variant:**

`governor.rs`
```
GovernorEvent::RequestPeerSharing(SocketAddr, u8)
```

**Node wiring:**

`node/mod.rs` — Governor task handles `RequestPeerSharing` by spawning a
short-lived async task that calls `request_peers_from()` from `peersharing.rs`
and feeds results back to the `PeerManager` via `add_shared_peer()`.

**Cooldown tracking:**

`peer_manager.rs` — Add `last_peer_sharing_request: Option<Instant>` to
`PeerInfo`. The governor skips peers that have been asked within the current
churn interval.

**Key files changed:**
- `crates/torsten-network/src/governor.rs`
- `crates/torsten-node/src/node/mod.rs`
- `crates/torsten-network/src/peer_manager.rs`

---

### Phase 5: Target Count Enforcement for Known Peers + BLP Preference During Sync

**Effort:** Medium (3-4 days)
**Issue coverage:** #138 (known target), #156 (BLP preference)

This phase completes the BLP priority story and adds true known-peer-count
enforcement.

**Part A: Known peer target enforcement**

Currently `try_evict_cold_peer()` is called reactively when a new peer is
added and the table is at capacity. The governor should proactively evict cold
peers when the known count exceeds `targets.known_peers`.

`governor.rs` — Add `evaluate_known_surplus(pm, targets)` → `Vec<GovernorEvent>`:
- New event: `GovernorEvent::EvictColdPeer(SocketAddr)`
- Emit up to N evictions per cycle (default 3) to avoid large discrete drops.
- Never evict local-root peers.
- Prefer evicting cold peers with the highest failure count and lowest reputation.

`node/mod.rs` — Handle `EvictColdPeer` by calling `pm.remove_cold_peer(addr)`.

**Part B: BLP preference during sync**

When `SyncState::Syncing`, the governor should not merely promote warm BLPs but
should actively evict non-BLP warm/hot peers down to a lower threshold to make
room for BLPs. This matches the Haskell governor's "use big ledger peers for
genesis lite sync" policy.

`governor.rs` — During `SyncState::Syncing` evaluation:
- If active-BLP count < `targets.active_blp` AND total hot count >= `targets.active_peers`:
  - Demote the lowest-reputation non-BLP hot peer to create a hot slot for a BLP.
- If established-BLP count < `targets.established_blp` AND total warm count >= threshold:
  - Disconnect the lowest-reputation non-BLP warm peer to create a slot for a BLP.

This ensures BLPs are not just promoted when slots happen to be available but
are actively prioritised during sync.

**Key files changed:**
- `crates/torsten-network/src/governor.rs`
- `crates/torsten-network/src/peer_manager.rs` (`remove_cold_peer`)
- `crates/torsten-node/src/node/mod.rs`

---

## 5. Architecture Notes

### Dependency Constraints

All governor/peer-manager changes stay within `torsten-network`. The node binary
(`torsten-node`) only interacts via the public `GovernorEvent` enum and the
`PeerManager` public API. No Cardano-domain types should enter `governor.rs` or
`peer_manager.rs`.

### Thread Safety Model

The `PeerManager` is wrapped in `Arc<RwLock<PeerManager>>` in the node.
The governor task acquires a read lock for `evaluate()` and a write lock only
for event application. This must remain true after all phases — no phase should
require the governor to hold a write lock during network I/O.

Phase 4 (peer sharing requests) introduces async work inside the governor task.
This must be structured as fire-and-forget spawned tasks, not inline awaits on
the governor loop, to keep the write-lock window minimal.

### Testing Strategy

Each phase must include unit tests that do not require network access:
- Phase 1: Test that `evaluate_blp()` emits `Connect` for cold BLPs at deficit,
  and that `local_root_deficit()` returns the correct addresses.
- Phase 2: Test that `check_transition_timeouts()` emits `Demote`/`Disconnect`
  for peers that have been in transitioning states beyond their timeout.
- Phase 3: Test that churn-selected peers are not immediately re-selected, and
  that local roots are never in the churned set.
- Phase 4: Test that `evaluate_peer_sharing()` emits `RequestPeerSharing` when
  below known-peer target and suppresses requests to peers asked recently.
- Phase 5: Test that known-peer surplus triggers `EvictColdPeer` respecting
  local-root protection, and that BLP preference during sync demotes non-BLPs
  when BLP targets are unmet.

### Rollout Sequencing

Phases 1 → 2 → 3 → 4 → 5 is the recommended order because:
- Phase 1 fixes the highest-severity silent bug and is independently valuable.
- Phase 2 provides the state-machine foundation that phases 3-5 build on for
  correct timeout semantics.
- Phase 3 is a contained change to `governor.rs` only.
- Phases 4 and 5 both add new `GovernorEvent` variants and touch more of the
  node wiring, so they benefit from phase 2 being stable first.

---

## 6. Files Summary

| File | Phases |
|---|---|
| `crates/torsten-network/src/governor.rs` | 1, 2, 3, 4, 5 |
| `crates/torsten-network/src/peer_manager.rs` | 1, 2, 4, 5 |
| `crates/torsten-node/src/config.rs` | 1 |
| `crates/torsten-node/src/node/mod.rs` | 1, 2, 4, 5 |
| `crates/torsten-network/src/client.rs` | 2 |
