# P2P Governor and Config Gaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close all P2P governor and config gaps vs Haskell cardano-node 10.x, covering config default/type corrections, startup validation, governor promotion/demotion exclusions, per-group diffusion mode wiring, schema alignment, and advertise filtering.

**Architecture:** Fix config defaults and types first (foundational), then complete startup validation, then fix governor aggregate paths to exclude topology+BLP peers, add the `aboveTargetLocal` demotion exception, wire per-group diffusion mode to handshakes, update dugite-config schema, and integrate advertise filtering in peer sharing.

**Tech Stack:** Rust, serde, cargo nextest

---

### Task 1: Fix config timeout types from u64 to f64

**Files:**
- Modify: `crates/dugite-node/src/config.rs:268-279` (field types)
- Modify: `crates/dugite-node/src/config.rs:384-394` (default functions)

Haskell uses `DiffTime` (picosecond-precision Rational) for all timeout fields. Our `u64` rejects valid fractional values from real cardano-node configs.

- [ ] **Step 1: Update default helper function return types**

```rust
fn default_protocol_idle_timeout() -> f64 {
    5.0
}

fn default_time_wait_timeout() -> f64 {
    60.0
}

fn default_egress_poll_interval() -> f64 {
    0.0
}
```

- [ ] **Step 2: Update NodeConfig field types**

In the `NodeConfig` struct, change:

```rust
    /// Time before idle mini-protocol connection is pruned (seconds, default: 5).
    #[serde(default = "default_protocol_idle_timeout")]
    pub protocol_idle_timeout: f64,
    /// Connection TIME_WAIT duration after close (seconds, default: 60).
    #[serde(default = "default_time_wait_timeout")]
    pub time_wait_timeout: f64,
    /// Outbound governor poll interval (seconds, default: 0).
    #[serde(default = "default_egress_poll_interval")]
    pub egress_poll_interval: f64,
    /// ChainSync-specific idle timeout (seconds, 0 = no timeout, absent = 3373s default).
    #[serde(default)]
    pub chain_sync_idle_timeout: Option<f64>,
```

- [ ] **Step 3: Update the Default impl**

In `impl Default for NodeConfig`, the fields already reference the helper functions so they pick up the new return types automatically. Update the `NodeConfig::default()` literal values only if they're inline — they use the helper functions, so no change needed here.

- [ ] **Step 4: Fix all compilation errors from u64→f64 change**

Search the workspace for usages of `protocol_idle_timeout`, `time_wait_timeout`, `egress_poll_interval`, `chain_sync_idle_timeout` and fix any type mismatches. These fields may be used in `Duration::from_secs()` calls — change to `Duration::from_secs_f64()`.

Run: `cargo build --all-targets 2>&1 | head -50`

- [ ] **Step 5: Update existing test assertions**

In `test_connection_timeouts_from_json` (config.rs ~line 969):

```rust
    #[test]
    fn test_connection_timeouts_from_json() {
        let json = r#"{
            "ProtocolIdleTimeout": 10,
            "TimeWaitTimeout": 120,
            "EgressPollInterval": 20
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!((config.protocol_idle_timeout - 10.0).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 120.0).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 20.0).abs() < f64::EPSILON);
    }
```

In `test_new_config_fields_absent_use_defaults` (config.rs ~line 984):

```rust
        assert!((config.protocol_idle_timeout - 5.0).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 60.0).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 0.0).abs() < f64::EPSILON);
```

- [ ] **Step 6: Add test for fractional timeout values**

```rust
    #[test]
    fn test_connection_timeouts_fractional() {
        let json = r#"{
            "ProtocolIdleTimeout": 5.5,
            "TimeWaitTimeout": 60.25,
            "EgressPollInterval": 0.1,
            "ChainSyncIdleTimeout": 3373.5
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        assert!((config.protocol_idle_timeout - 5.5).abs() < f64::EPSILON);
        assert!((config.time_wait_timeout - 60.25).abs() < f64::EPSILON);
        assert!((config.egress_poll_interval - 0.1).abs() < f64::EPSILON);
        assert!((config.chain_sync_idle_timeout.unwrap() - 3373.5).abs() < f64::EPSILON);
    }
```

- [ ] **Step 7: Run tests, verify pass**

Run: `cargo nextest run -p dugite-node -E 'test(timeout)' -E 'test(config)'`

- [ ] **Step 8: Commit**

```
git add -A && git commit -m "fix: change timeout config fields from u64 to f64

Haskell's DiffTime accepts fractional seconds (5.5, 0.1, etc.).
Our u64 fields rejected valid cardano-node config values.
Also fix EgressPollInterval default from 10 to 0."
```

---

### Task 2: Fix config default values to match Haskell

**Files:**
- Modify: `crates/dugite-node/src/config.rs:629-658` (Default impl)
- Modify: `crates/dugite-node/src/config.rs:368-382` (sync default helpers)

- [ ] **Step 1: Fix deadline target defaults in Default impl**

In `impl Default for NodeConfig`, change:

```rust
            target_number_of_active_peers: 20,
            target_number_of_established_peers: 30,
            target_number_of_known_peers: 150,
```

(was: active=15, established=40, known=85)

- [ ] **Step 2: Fix sync target defaults in Default impl**

```rust
            sync_target_number_of_active_peers: 5,
            sync_target_number_of_established_peers: 10,
            sync_target_number_of_known_peers: 150,
```

(was: all 0)

- [ ] **Step 3: Fix sync BLP established default helper**

```rust
fn default_sync_established_blp() -> usize {
    40
}
```

(was: 50)

- [ ] **Step 4: Update test assertions for new defaults**

In `test_new_config_fields_absent_use_defaults`:

```rust
        assert_eq!(
            config.sync_target_number_of_established_big_ledger_peers,
            40
        );
        assert!((config.egress_poll_interval - 0.0).abs() < f64::EPSILON);
```

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo nextest run -p dugite-node -E 'test(config)'`

- [ ] **Step 6: Commit**

```
git add -A && git commit -m "fix: correct config defaults to match Haskell cardano-node 10.x

Deadline targets: known 85→150, established 40→30, active 15→20
(source: defaultDeadlineTargets(Relay) in Configuration.hs)
Sync targets: active 0→5, established 0→10, known 0→150
Sync established BLP: 50→40
(source: defaultSyncTargets in Configuration.hs)"
```

---

### Task 3: Fix AcceptedConnectionsLimit JSON key names

**Files:**
- Modify: `crates/dugite-node/src/config.rs:22-46` (struct + serde attrs)
- Modify: `crates/dugite-node/src/config.rs:949-965` (test)

Haskell's hand-written `FromJSON` uses short keys: `hardLimit`, `softLimit`, `delay`. Our `#[serde(rename_all = "camelCase")]` on the full field names produces `acceptedConnectionsHardLimit` — wrong.

- [ ] **Step 1: Replace serde rename strategy with per-field renames**

```rust
/// Inbound connection limits (matches Haskell AcceptedConnectionsLimit).
///
/// Haskell's hand-written FromJSON (OrphanInstances.hs) uses short keys:
/// `{"hardLimit": 512, "softLimit": 384, "delay": 5}`
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AcceptedConnectionsLimit {
    /// Refuse new inbound connections beyond this count.
    #[serde(alias = "acceptedConnectionsHardLimit", rename = "hardLimit", default = "default_hard_limit")]
    pub hard_limit: u32,
    /// Start delaying new connections at this count.
    #[serde(alias = "acceptedConnectionsSoftLimit", rename = "softLimit", default = "default_soft_limit")]
    pub soft_limit: u32,
    /// Max delay in seconds applied to connections above soft limit.
    /// Scales linearly from 0 at soft limit to this value at hard limit.
    #[serde(alias = "acceptedConnectionsDelay", rename = "delay", default = "default_conn_delay")]
    pub delay: f64,
}

impl Default for AcceptedConnectionsLimit {
    fn default() -> Self {
        Self {
            hard_limit: 512,
            soft_limit: 384,
            delay: 5.0,
        }
    }
}
```

Note: `alias` preserves backward compatibility with our old key names. `rename` makes serialization use the Haskell-matching short keys.

- [ ] **Step 2: Fix default_conn_delay return type**

```rust
fn default_conn_delay() -> f64 {
    5.0
}
```

(was: `u32`, returning 5)

- [ ] **Step 3: Fix all compilation errors from field rename**

Search for `.accepted_connections_hard_limit`, `.accepted_connections_soft_limit`, `.accepted_connections_delay` throughout the codebase and rename to `.hard_limit`, `.soft_limit`, `.delay`.

Run: `cargo build --all-targets 2>&1 | head -50`

- [ ] **Step 4: Update test to use Haskell-matching short keys**

```rust
    #[test]
    fn test_accepted_connections_limit_from_json() {
        let json = r#"{
            "AcceptedConnectionsLimit": {
                "hardLimit": 256,
                "softLimit": 200,
                "delay": 2
            }
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        let limit = config.accepted_connections_limit.unwrap();
        assert_eq!(limit.hard_limit, 256);
        assert_eq!(limit.soft_limit, 200);
        assert!((limit.delay - 2.0).abs() < f64::EPSILON);
    }
```

- [ ] **Step 5: Add backward-compat test for old key names**

```rust
    #[test]
    fn test_accepted_connections_limit_old_keys() {
        // Our old camelCase keys should still parse via alias.
        let json = r#"{
            "AcceptedConnectionsLimit": {
                "acceptedConnectionsHardLimit": 256,
                "acceptedConnectionsSoftLimit": 200,
                "acceptedConnectionsDelay": 2
            }
        }"#;
        let config: NodeConfig = serde_json::from_str(json).unwrap();
        let limit = config.accepted_connections_limit.unwrap();
        assert_eq!(limit.hard_limit, 256);
        assert_eq!(limit.soft_limit, 200);
    }
```

- [ ] **Step 6: Run tests, verify pass**

Run: `cargo nextest run -p dugite-node -E 'test(accepted_connections)'`

- [ ] **Step 7: Commit**

```
git add -A && git commit -m "fix: AcceptedConnectionsLimit uses Haskell short keys

Haskell's FromJSON uses 'hardLimit', 'softLimit', 'delay' — not the
full camelCase field names. Add serde aliases for backward compat
with our old key format. Change delay type from u32 to f64."
```

---

### Task 4: Complete startup validation — `sane_peer_selection_targets`

**Files:**
- Modify: `crates/dugite-node/src/config.rs:502-567` (validate method)

Replace the existing incomplete validation with a function matching all 14 Haskell predicates, applied unconditionally to both deadline and sync targets.

- [ ] **Step 1: Add the `sane_peer_selection_targets` helper function**

Add this before `impl NodeConfig`:

```rust
/// Validate peer selection targets matching Haskell's `sanePeerSelectionTargets`.
///
/// Checks all 14 predicates from `Governor/Types.hs`:
/// - Ordering: 0 <= active <= established <= known, root <= known
/// - Upper bounds: active <= 100, established <= 1000, known <= 10000
/// - Same constraints for big ledger peer targets
fn sane_peer_selection_targets(
    label: &str,
    active: usize,
    established: usize,
    known: usize,
    root: usize,
    active_blp: usize,
    established_blp: usize,
    known_blp: usize,
) -> Result<()> {
    // Regular peer ordering.
    if active > established {
        anyhow::bail!(
            "{label}: TargetNumberOfActivePeers ({active}) must be <= \
             TargetNumberOfEstablishedPeers ({established})"
        );
    }
    if established > known {
        anyhow::bail!(
            "{label}: TargetNumberOfEstablishedPeers ({established}) must be <= \
             TargetNumberOfKnownPeers ({known})"
        );
    }
    if root > known {
        anyhow::bail!(
            "{label}: TargetNumberOfRootPeers ({root}) must be <= \
             TargetNumberOfKnownPeers ({known})"
        );
    }

    // BLP ordering.
    if active_blp > established_blp {
        anyhow::bail!(
            "{label}: TargetNumberOfActiveBigLedgerPeers ({active_blp}) must be <= \
             TargetNumberOfEstablishedBigLedgerPeers ({established_blp})"
        );
    }
    if established_blp > known_blp {
        anyhow::bail!(
            "{label}: TargetNumberOfEstablishedBigLedgerPeers ({established_blp}) must be <= \
             TargetNumberOfKnownBigLedgerPeers ({known_blp})"
        );
    }

    // Upper bounds — regular.
    if active > 100 {
        anyhow::bail!("{label}: TargetNumberOfActivePeers ({active}) must be <= 100");
    }
    if established > 1000 {
        anyhow::bail!("{label}: TargetNumberOfEstablishedPeers ({established}) must be <= 1000");
    }
    if known > 10000 {
        anyhow::bail!("{label}: TargetNumberOfKnownPeers ({known}) must be <= 10000");
    }

    // Upper bounds — BLP.
    if active_blp > 100 {
        anyhow::bail!("{label}: TargetNumberOfActiveBigLedgerPeers ({active_blp}) must be <= 100");
    }
    if established_blp > 1000 {
        anyhow::bail!(
            "{label}: TargetNumberOfEstablishedBigLedgerPeers ({established_blp}) must be <= 1000"
        );
    }
    if known_blp > 10000 {
        anyhow::bail!(
            "{label}: TargetNumberOfKnownBigLedgerPeers ({known_blp}) must be <= 10000"
        );
    }

    Ok(())
}
```

- [ ] **Step 2: Replace existing validation in `validate()`**

Replace the peer-target validation section (lines 503-567) with:

```rust
        // ── Peer target validation (matches Haskell sanePeerSelectionTargets) ──
        // Applied unconditionally to both deadline and sync target sets.
        sane_peer_selection_targets(
            "deadline",
            self.target_number_of_active_peers,
            self.target_number_of_established_peers,
            self.target_number_of_known_peers,
            self.target_number_of_root_peers,
            self.target_number_of_active_big_ledger_peers,
            self.target_number_of_established_big_ledger_peers,
            self.target_number_of_known_big_ledger_peers,
        )?;
        sane_peer_selection_targets(
            "sync",
            self.sync_target_number_of_active_peers,
            self.sync_target_number_of_established_peers,
            self.sync_target_number_of_known_peers,
            self.sync_target_number_of_root_peers,
            self.sync_target_number_of_active_big_ledger_peers,
            self.sync_target_number_of_established_big_ledger_peers,
            self.sync_target_number_of_known_big_ledger_peers,
        )?;
```

Note: sync targets validated unconditionally — no `consensus_mode` guard.

- [ ] **Step 3: Update existing tests**

The test `test_validate_sync_targets_skipped_in_praos_mode` should now FAIL (sync targets are validated unconditionally). Delete it or change it to expect failure:

```rust
    #[test]
    fn test_validate_sync_targets_always_checked() {
        // Haskell validates sync targets unconditionally, not just in GenesisMode.
        let config = NodeConfig {
            consensus_mode: ConsensusMode::PraosMode,
            sync_target_number_of_known_peers: 5,
            sync_target_number_of_established_peers: 10,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("sync"));
    }
```

- [ ] **Step 4: Add tests for new constraint types**

```rust
    #[test]
    fn test_validate_root_exceeds_known_fails() {
        let config = NodeConfig {
            target_number_of_root_peers: 200,
            target_number_of_known_peers: 150,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("RootPeers"));
    }

    #[test]
    fn test_validate_active_exceeds_100_fails() {
        let config = NodeConfig {
            target_number_of_active_peers: 101,
            target_number_of_established_peers: 1000,
            target_number_of_known_peers: 10000,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("ActivePeers"));
        assert!(err.to_string().contains("100"));
    }

    #[test]
    fn test_validate_blp_upper_bounds() {
        let config = NodeConfig {
            target_number_of_active_big_ledger_peers: 101,
            ..NodeConfig::default()
        };
        let err = config.validate(Path::new(".")).unwrap_err();
        assert!(err.to_string().contains("BigLedgerPeers"));
    }

    #[test]
    fn test_validate_default_config_passes() {
        let config = NodeConfig::default();
        assert!(config.validate(Path::new(".")).is_ok());
    }
```

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo nextest run -p dugite-node -E 'test(validate)'`

- [ ] **Step 6: Commit**

```
git add -A && git commit -m "fix: complete startup validation matching Haskell sanePeerSelectionTargets

All 14 predicates now checked for both deadline and sync targets.
Sync targets validated unconditionally (not just in GenesisMode).
Added: root<=known, upper bounds (active<=100, established<=1000,
known<=10000) for both regular and BLP target sets."
```

---

### Task 5: Exclude topology and BLP peers from aggregate promotion paths

**Files:**
- Modify: `crates/dugite-network/src/peer/governor.rs:237-273` (Stage 2 and 3)

- [ ] **Step 1: Write failing test — aggregate path must not select topology peers**

In `governor.rs` tests:

```rust
    #[test]
    fn aggregate_warm_promotion_excludes_topology_peers() {
        let mut pm = PeerManager::new();
        // Topology peer (cold, eligible) — should NOT be promoted by aggregate path.
        pm.add_peer(test_addr(3001), PeerSource::Topology);
        // Ledger peer (cold, eligible) — should be promoted.
        pm.add_peer(test_addr(3002), PeerSource::Ledger);

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 2,
                target_hot: 0,
                max_cold: 100,
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &[]);

        // Only the ledger peer should be promoted.
        let promoted: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::PromoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert!(promoted.contains(&test_addr(3002)));
        assert!(!promoted.contains(&test_addr(3001)));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p dugite-network -E 'test(aggregate_warm_promotion_excludes)'`
Expected: FAIL — topology peer is currently promoted by aggregate path.

- [ ] **Step 3: Add topology+BLP exclusion to Stage 2 (cold→warm)**

In `governor.rs`, modify Stage 2 (~line 243-254). After getting `cold_peers`, add a filter:

```rust
        // Promote cold → warm if below target.
        // Exclude topology peers (managed by per-group belowTargetLocal above)
        // and big ledger peers (future: dedicated BLP promotion path).
        if warm_count + hot_count < self.config.targets.target_warm {
            let needed = self.config.targets.target_warm - (warm_count + hot_count);
            let cold_peers = peer_manager.peers_eligible_to_connect();
            let mut promoted = 0;
            for &addr in &cold_peers {
                if promoted >= needed {
                    break;
                }
                if already_promoted.contains(&addr) {
                    continue;
                }
                // Exclude topology and BLP peers — they have dedicated paths.
                if let Some(info) = peer_manager.get_peer(&addr) {
                    if info.source == PeerSource::Topology {
                        continue;
                    }
                }
                actions.push(GovernorAction::PromoteToWarm(addr));
                already_promoted.insert(addr);
                promoted += 1;
            }
        }
```

Note: add `use super::manager::PeerSource;` at the top of `compute_actions` if not already in scope for this block.

- [ ] **Step 4: Add topology+BLP exclusion to Stage 3 (warm→hot)**

Modify Stage 3 (~line 257-273):

```rust
        // Promote warm → hot if below target.
        // Exclude topology peers (managed by per-group belowTargetLocal)
        // and big ledger peers (future: dedicated BLP promotion path).
        if hot_count < self.config.targets.target_hot {
            let needed = self.config.targets.target_hot - hot_count;
            let warm_peers = peer_manager.peers_in_state(PeerState::Warm);
            let mut promoted = 0;
            for &addr in &warm_peers {
                if promoted >= needed {
                    break;
                }
                if already_promoted.contains(&addr) {
                    continue;
                }
                // Exclude topology and BLP peers — they have dedicated paths.
                if let Some(info) = peer_manager.get_peer(&addr) {
                    if info.source == PeerSource::Topology {
                        continue;
                    }
                }
                actions.push(GovernorAction::PromoteToHot(addr));
                already_promoted.insert(addr);
                promoted += 1;
            }
        }
```

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo nextest run -p dugite-network -E 'test(aggregate)'`
Expected: PASS

- [ ] **Step 6: Commit**

```
git add -A && git commit -m "fix: exclude topology peers from aggregate promotion paths

Matches Haskell's belowTargetOther which explicitly excludes
LocalRootPeers.keysSet. Topology peers are now only promoted
by the per-group belowTargetLocal logic (Stage 1)."
```

---

### Task 6: Add `in_progress_promote_cold` and `in_progress_promote_warm` tracking

**Files:**
- Modify: `crates/dugite-network/src/peer/governor.rs:115-132` (Governor struct)
- Modify: `crates/dugite-network/src/peer/governor.rs:139-365` (compute_actions)

- [ ] **Step 1: Add in-progress tracking fields to Governor**

```rust
pub struct Governor {
    config: GovernorConfig,
    last_hot_churn: Instant,
    last_cold_churn: Instant,
    last_warm_churn: Instant,
    /// Peers with in-flight cold→warm promotion (prevents double-promotion
    /// across governor ticks). Matches Haskell's `inProgressPromoteCold`.
    in_progress_promote_cold: HashSet<SocketAddr>,
    /// Peers with in-flight warm→hot promotion. Matches Haskell's
    /// `inProgressPromoteWarm`.
    in_progress_promote_warm: HashSet<SocketAddr>,
}
```

Initialize both as `HashSet::new()` in `Governor::new()`.

- [ ] **Step 2: Add methods to clear in-progress tracking**

```rust
    /// Mark a cold→warm promotion as completed (success or failure).
    /// Called by the connection manager when the promotion job finishes.
    pub fn promotion_cold_completed(&mut self, addr: &SocketAddr) {
        self.in_progress_promote_cold.remove(addr);
    }

    /// Mark a warm→hot promotion as completed (success or failure).
    pub fn promotion_warm_completed(&mut self, addr: &SocketAddr) {
        self.in_progress_promote_warm.remove(addr);
    }
```

- [ ] **Step 3: Subtract in-progress sets from candidate pools in compute_actions**

In Stage 1 (per-group cold→warm), after the `eligible_to_connect` check, add:

```rust
                    if self.in_progress_promote_cold.contains(addr) {
                        continue;
                    }
```

In Stage 1 (per-group warm→hot), add:

```rust
                    if self.in_progress_promote_warm.contains(addr) {
                        continue;
                    }
```

Apply the same filters in Stage 2 and Stage 3 aggregate paths.

- [ ] **Step 4: Record in-progress when emitting promotion actions**

After each `actions.push(GovernorAction::PromoteToWarm(addr))`:

```rust
                    self.in_progress_promote_cold.insert(addr);
```

After each `actions.push(GovernorAction::PromoteToHot(addr))`:

```rust
                    self.in_progress_promote_warm.insert(addr);
```

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo nextest run -p dugite-network`

- [ ] **Step 6: Commit**

```
git add -A && git commit -m "feat: add in-progress promotion tracking to Governor

Matches Haskell's inProgressPromoteCold and inProgressPromoteWarm
sets. Prevents double-promotion across governor ticks when async
promotion jobs are still in flight."
```

---

### Task 7: Add `above_target_local_hot` — demote excess local root hot peers

**Files:**
- Modify: `crates/dugite-network/src/peer/governor.rs` (after Stage 4, ~line 300)
- Modify: `crates/dugite-network/src/peer/selection.rs` (new function)

- [ ] **Step 1: Write failing test**

In `governor.rs` tests:

```rust
    #[test]
    fn above_target_local_hot_demotes_excess() {
        let mut pm = PeerManager::new();
        // Group with hot_valency=1 but 2 hot members.
        pm.add_peer(test_addr(3001), PeerSource::Topology);
        pm.promote_to_warm(&test_addr(3001));
        pm.promote_to_hot(&test_addr(3001));
        pm.get_peer_mut(&test_addr(3001))
            .unwrap()
            .update_latency(10.0); // good score

        pm.add_peer(test_addr(3002), PeerSource::Topology);
        pm.promote_to_warm(&test_addr(3002));
        pm.promote_to_hot(&test_addr(3002));
        pm.get_peer_mut(&test_addr(3002))
            .unwrap()
            .update_latency(500.0); // worse score

        let groups = vec![LocalRootGroupTarget {
            members: [test_addr(3001), test_addr(3002)]
                .into_iter()
                .collect(),
            warm_valency: 2,
            hot_valency: 1, // Only 1 hot wanted, but 2 are hot.
        }];

        let config = GovernorConfig {
            targets: PeerTargets {
                target_warm: 10,
                target_hot: 10,
                max_cold: 100,
            },
            ..GovernorConfig::default()
        };
        let mut gov = Governor::new(config);
        let actions = gov.compute_actions(&pm, &groups);

        // Should demote exactly 1 — the worst-scoring hot member.
        let demoted: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                GovernorAction::DemoteToWarm(addr) => Some(*addr),
                _ => None,
            })
            .collect();
        assert_eq!(demoted.len(), 1);
        assert_eq!(demoted[0], test_addr(3002)); // worse score
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p dugite-network -E 'test(above_target_local_hot)'`
Expected: FAIL — no excess demotions emitted.

- [ ] **Step 3: Implement `above_target_local_hot` in governor.rs**

Add this block after Stage 4 (the existing hot→warm demotion for non-topology peers), before the hot churn section:

```rust
        // ── aboveTargetLocal hot→warm for local roots ──────────────────
        // The ONLY path that can demote local root peers: when a group has
        // MORE hot members than its hotValency. Matches Haskell's
        // ActivePeers.aboveTargetLocal.
        {
            use super::selection::peer_score;

            for group in local_root_groups {
                let mut hot_members: Vec<(SocketAddr, f64)> = group
                    .members
                    .iter()
                    .filter_map(|addr| {
                        peer_manager.get_peer(addr).and_then(|info| {
                            if info.state == PeerState::Hot {
                                Some((*addr, peer_score(info)))
                            } else {
                                None
                            }
                        })
                    })
                    .collect();

                if hot_members.len() > group.hot_valency {
                    let excess = hot_members.len() - group.hot_valency;
                    // Sort ascending by score — worst first.
                    hot_members.sort_by(|(_, a), (_, b)| {
                        a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    for (addr, _) in hot_members.into_iter().take(excess) {
                        actions.push(GovernorAction::DemoteToWarm(addr));
                    }
                }
            }
        }
```

- [ ] **Step 4: Run tests, verify pass**

Run: `cargo nextest run -p dugite-network -E 'test(above_target_local_hot)'`
Expected: PASS

- [ ] **Step 5: Commit**

```
git add -A && git commit -m "feat: add aboveTargetLocal hot→warm demotion for local roots

When a local root group exceeds its hotValency (e.g. from inbound
connections), demote the worst-scoring excess members. This is the
only path that can demote topology peers, matching Haskell's
ActivePeers.aboveTargetLocal."
```

---

### Task 8: Wire per-group `diffusionMode` to handshake

**Files:**
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs:300-326,370-382,412-428`

The `ConnectionLifecycleManager` stores a single `initiator_only: bool` at construction and uses it for all outbound connections. We need to make it per-connection based on `effective_diffusion_mode()`.

- [ ] **Step 1: Add `NodePeerManager` access to `ConnectionLifecycleManager`**

The peer sharing server task already has `peer_manager_for_servers` (an `Arc<RwLock<NodePeerManager>>`). The lifecycle manager needs the same to query `effective_diffusion_mode()`. Add a field or closure for per-peer diffusion mode lookup. The simplest approach: change `connect()` and `spawn_connect()` to accept an `initiator_only` parameter instead of using the stored field.

In `connect()` (~line 376), change:

```rust
    pub async fn connect(
        &mut self,
        addr: SocketAddr,
        initiator_only: bool,
    ) -> Result<PeerConnection, ConnectionError> {
        let mut conn = PeerConnection::connect(
            addr,
            self.network_magic,
            initiator_only,
            self.peer_sharing,
            Some(self.connect_timeout),
        )
        .await?;
```

In `spawn_connect()` (~line 412), change:

```rust
    pub fn spawn_connect(&self, addr: SocketAddr, initiator_only: bool, tx: mpsc::Sender<ConnectResult>) {
        let network_magic = self.network_magic;
        let peer_sharing = self.peer_sharing;
        let connect_timeout = self.connect_timeout;
        let metrics = Arc::clone(&self.metrics);

        tokio::spawn(async move {
            match PeerConnection::connect(
                addr,
                network_magic,
                initiator_only,
                peer_sharing,
                Some(connect_timeout),
            )
```

- [ ] **Step 2: Update all callsites to pass per-peer `initiator_only`**

Find all calls to `connect()` and `spawn_connect()` in `connection_lifecycle.rs` and the node orchestration code. At each callsite, compute the per-peer value:

```rust
let initiator_only = {
    let pm = peer_manager.read().await;
    pm.effective_diffusion_mode(&addr) == DiffusionMode::InitiatorOnly
};
```

For `spawn_connect` calls where `peer_manager` is available, do the same before spawning.

- [ ] **Step 3: Fix compilation errors**

Run: `cargo build --all-targets 2>&1 | head -80`

Fix any remaining callsite mismatches.

- [ ] **Step 4: Run full test suite**

Run: `cargo nextest run --workspace`

- [ ] **Step 5: Commit**

```
git add -A && git commit -m "feat: wire per-group diffusionMode to outbound handshake

effective_diffusion_mode() was already implemented but never called.
Now each outbound connection uses the per-group diffusion mode from
the topology, falling back to the global DiffusionMode config."
```

---

### Task 9: Update dugite-config schema defaults

**Files:**
- Modify: `crates/dugite-config/src/schema.rs:839-860,916` (default_config_for_network)

- [ ] **Step 1: Fix deadline target defaults in schema**

In `default_config_for_network()` (~line 839):

```rust
    map.insert("TargetNumberOfActivePeers".into(), json!(20));
    map.insert("TargetNumberOfEstablishedPeers".into(), json!(30));
    map.insert("TargetNumberOfKnownPeers".into(), json!(150));
```

(was: 15, 40, 85)

- [ ] **Step 2: Fix sync target defaults in schema**

```rust
    map.insert("SyncTargetNumberOfActivePeers".into(), json!(5));
    map.insert("SyncTargetNumberOfEstablishedPeers".into(), json!(10));
    map.insert("SyncTargetNumberOfKnownPeers".into(), json!(150));
    map.insert("SyncTargetNumberOfRootPeers".into(), json!(0));
    map.insert("SyncTargetNumberOfActiveBigLedgerPeers".into(), json!(30));
    map.insert(
        "SyncTargetNumberOfEstablishedBigLedgerPeers".into(),
        json!(40),
    );
    map.insert("SyncTargetNumberOfKnownBigLedgerPeers".into(), json!(100));
```

- [ ] **Step 3: Fix EgressPollInterval default**

```rust
    map.insert("EgressPollInterval".into(), json!(0));
```

(was: 10)

- [ ] **Step 4: Run dugite-config tests**

Run: `cargo nextest run -p dugite-config`

- [ ] **Step 5: Commit**

```
git add -A && git commit -m "fix: update dugite-config schema defaults to match Haskell

Deadline targets: known=150, established=30, active=20
Sync targets: active=5, established=10, known=150, estBLP=40
EgressPollInterval: 0 (was 10)"
```

---

### Task 10: Integrate advertise flag in peer sharing responses

**Files:**
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs:1317-1321` (peer list construction)

- [ ] **Step 1: Filter out non-advertisable peers in the peer sharing server task**

In `make_peersharing_server_task` (~line 1317), change:

```rust
                let peers: Vec<SocketAddr> = {
                    let pm = peer_manager.read().await;
                    pm.connected_peer_addrs()
                        .into_iter()
                        .filter(|addr| pm.is_advertisable(addr))
                        .collect()
                };
```

- [ ] **Step 2: Run full test suite**

Run: `cargo nextest run --workspace`

- [ ] **Step 3: Commit**

```
git add -A && git commit -m "fix: filter non-advertisable peers from PeerSharing responses

Peers in local root groups with advertise=false are now excluded
from PeerSharing responses. is_advertisable() already existed but
was not integrated into the response construction path."
```

---

### Task 11: Final verification — clippy, fmt, full test suite

**Files:** None (verification only)

- [ ] **Step 1: Run cargo fmt**

Run: `cargo fmt --all -- --check`

If it fails, run `cargo fmt --all` and commit the formatting changes.

- [ ] **Step 2: Run cargo clippy**

Run: `cargo clippy --all-targets -- -D warnings`

Fix any warnings.

- [ ] **Step 3: Run full test suite**

Run: `cargo nextest run --workspace`

All tests must pass.

- [ ] **Step 4: Run doc tests**

Run: `cargo test --doc`

- [ ] **Step 5: Final commit (if any fixes needed)**

```
git add -A && git commit -m "chore: fix clippy warnings and formatting"
```
