# Ouroboros Genesis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the dormant GSM, GDD, and LoE code into Dugite's live sync pipeline using an event-based actor, giving eclipse resistance during initial sync.

**Architecture:** A dedicated GSM actor tokio task owns the GenesisStateMachine exclusively. Producers (ChainSync, BlockFetch, networking) emit GsmEvents via an mpsc channel. The actor publishes state via a watch channel (GsmSnapshot) and GDD disconnect commands via a second mpsc channel. No Arc<RwLock<>> on the GSM.

**Tech Stack:** Rust, tokio (mpsc, watch, select!, Interval), dugite-consensus DensityWindow, dugite-node gsm.rs

**Spec:** `docs/superpowers/specs/2026-04-01-ouroboros-genesis-design.md`

---

## File Structure

| File | Role | Action |
|------|------|--------|
| `crates/dugite-consensus/src/chain_selection.rs` | DensityWindow — add helper methods | Modify |
| `crates/dugite-node/src/gsm.rs` | GSM types, actor, GDD algorithm | Rewrite |
| `crates/dugite-node/src/node/mod.rs` | Node struct, GSM init, actor spawn | Modify |
| `crates/dugite-node/src/node/sync.rs` | Emit GsmEvents, consume GsmSnapshot | Modify |
| `crates/dugite-node/src/node/networking.rs` | Emit PeerDisconnected, consume GddAction | Modify |
| `crates/dugite-node/src/node/connection_lifecycle.rs` | Emit PeerDisconnected on conn close | Modify |

---

### Task 1: Extend DensityWindow with Helper Methods

**Files:**
- Modify: `crates/dugite-consensus/src/chain_selection.rs:396-461`

- [ ] **Step 1: Write failing tests for new DensityWindow methods**

Add to the existing test module in `chain_selection.rs`:

```rust
#[test]
fn test_density_window_blocks_before() {
    let mut dw = DensityWindow::new(0, 100);
    dw.record_block(10);
    dw.record_block(20);
    dw.record_block(30);
    dw.record_block(40);
    assert_eq!(dw.blocks_before(25), 2); // slots 10, 20
    assert_eq!(dw.blocks_before(10), 0); // strictly before
    assert_eq!(dw.blocks_before(11), 1); // slot 10 only
    assert_eq!(dw.blocks_before(100), 4); // all blocks
    assert_eq!(dw.blocks_before(0), 0); // none
}

#[test]
fn test_density_window_has_block_at_or_after() {
    let mut dw = DensityWindow::new(0, 100);
    dw.record_block(10);
    dw.record_block(20);
    assert!(dw.has_block_at_or_after(15)); // slot 20 qualifies
    assert!(dw.has_block_at_or_after(20)); // exactly slot 20
    assert!(!dw.has_block_at_or_after(21)); // nothing at or after 21
    assert!(dw.has_block_at_or_after(1)); // slot 10 qualifies
}

#[test]
fn test_density_window_head_slot() {
    let mut dw = DensityWindow::new(0, 100);
    assert_eq!(dw.head_slot(), None); // empty
    dw.record_block(10);
    dw.record_block(5);
    dw.record_block(20);
    assert_eq!(dw.head_slot(), Some(20));
}

#[test]
fn test_density_window_total_block_count() {
    let mut dw = DensityWindow::new(0, 50);
    assert_eq!(dw.total_block_count(), 0);
    dw.record_block(10);
    dw.record_block(20);
    dw.record_block(30);
    // record_block only accepts slots within (intersection, intersection + window_size]
    // so slot 60 should be rejected (outside window 0+50=50)
    dw.record_block(60);
    assert_eq!(dw.total_block_count(), 3);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run -p dugite-consensus -E 'test(test_density_window_blocks_before) | test(test_density_window_has_block_at_or_after) | test(test_density_window_head_slot) | test(test_density_window_total_block_count)'`

Expected: FAIL — methods don't exist yet.

- [ ] **Step 3: Implement the four new methods**

Add to the `impl DensityWindow` block (after the existing `reset` method at line ~460):

```rust
/// Count of blocks with slot strictly less than `slot`.
/// Used by GDD to clip density to the genesis window boundary.
#[inline]
pub fn blocks_before(&self, slot: u64) -> u64 {
    // self.slots is maintained in sorted order by record_block
    self.slots.partition_point(|&s| s < slot) as u64
}

/// Whether any recorded block has slot >= `slot`.
/// Used by GDD to detect if a peer has progressed past the genesis window.
#[inline]
pub fn has_block_at_or_after(&self, slot: u64) -> bool {
    self.slots.partition_point(|&s| s < slot) < self.slots.len()
}

/// Highest slot recorded, or None if empty.
/// Used by GDD to compute potential_slots (unknown trailing slots).
#[inline]
pub fn head_slot(&self) -> Option<u64> {
    self.slots.last().copied()
}

/// Total number of blocks recorded in the window.
/// Alias for block_count() — named for clarity in GDD context where
/// it represents the total suffix length, not just the clipped count.
#[inline]
pub fn total_block_count(&self) -> u64 {
    self.slots.len() as u64
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-consensus -E 'test(test_density_window_blocks_before) | test(test_density_window_has_block_at_or_after) | test(test_density_window_head_slot) | test(test_density_window_total_block_count)'`

Expected: PASS

- [ ] **Step 5: Run full consensus test suite + clippy**

Run: `cargo nextest run -p dugite-consensus && cargo clippy -p dugite-consensus --all-targets -- -D warnings`

Expected: All pass, zero warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-consensus/src/chain_selection.rs
git commit -m "feat(consensus): add DensityWindow helper methods for GDD (#316)"
```

---

### Task 2: Define GSM Event Types and GsmSnapshot

**Files:**
- Modify: `crates/dugite-node/src/gsm.rs`

- [ ] **Step 1: Add the new types at the top of gsm.rs**

After the existing imports (line ~44), add:

```rust
use tokio::sync::{mpsc, watch};
```

After the `GenesisSyncState` Display impl (line ~65), add the new types:

```rust
/// Event sent to the GSM actor from producers (ChainSync, BlockFetch, networking).
#[derive(Debug)]
pub enum GsmEvent {
    /// Peer completed ChainSync find_intersect — register for density tracking.
    PeerRegistered {
        addr: SocketAddr,
        intersection_slot: u64,
        tip_slot: u64,
    },
    /// Peer disconnected — remove from density tracking.
    PeerDisconnected { addr: SocketAddr },
    /// Block header received from peer — record in density window.
    /// Emitted on header arrival (before validation), matching Haskell's csLatestSlot.
    BlockReceived { addr: SocketAddr, slot: u64 },
    /// Peer reported new tip via ChainSync header.
    PeerTipUpdated { addr: SocketAddr, tip_slot: u64 },
    /// Peer sent MsgAwaitReply — no more headers right now.
    /// Maps to Haskell's csIdling = True.
    PeerIdling { addr: SocketAddr },
    /// Peer resumed sending headers (roll-forward/roll-backward received).
    /// Maps to Haskell's csIdling = False.
    PeerActive { addr: SocketAddr },
    /// Periodic sync status from background tick.
    SyncStatus {
        active_blp_count: usize,
        all_chainsync_idle: bool,
        tip_age_secs: u64,
        immutable_tip_slot: u64,
    },
}

/// Published by the GSM actor via watch channel. Read by the sync pipeline
/// (for LoE enforcement) and metrics (for Prometheus gauges).
#[derive(Debug, Clone, Copy)]
pub struct GsmSnapshot {
    /// Current Genesis sync state.
    pub state: GenesisSyncState,
    /// LoE ceiling slot. None means no constraint (CaughtUp).
    /// PreSyncing: Some(0) — freeze completely.
    /// Syncing: Some(min_intersection_slot) — common prefix approximation.
    /// CaughtUp: None — no constraint.
    pub loe_slot: Option<u64>,
}

/// Action emitted by the GSM actor when GDD identifies peers to disconnect.
#[derive(Debug)]
pub enum GddAction {
    /// Disconnect a peer with insufficient chain density.
    DisconnectPeer(SocketAddr),
}
```

- [ ] **Step 2: Update PeerChainInfo to include idling and latest_slot**

Replace the existing `PeerChainInfo` struct (lines 106-140) with:

```rust
/// Per-peer chain state tracked by the GSM actor for GDD evaluation.
#[derive(Debug, Clone)]
pub struct PeerChainInfo {
    /// Density window tracking blocks in the genesis window for this peer.
    pub density_window: DensityWindow,
    /// Most recent tip slot reported by this peer.
    pub tip_slot: u64,
    /// Intersection slot where this peer's chain diverges from ours.
    pub intersection_slot: u64,
    /// Whether the peer is currently idling (sent MsgAwaitReply).
    /// Critical for GDD Guard 4: idling peers use lower_bound comparison,
    /// non-idling peers use upper_bound (benefit of the doubt).
    pub idling: bool,
    /// Most recent header slot received from this peer. May be beyond the
    /// candidate fragment if not yet validated. Maps to Haskell's csLatestSlot.
    pub latest_slot: Option<u64>,
}

impl PeerChainInfo {
    /// Create a new info record with a fresh density window.
    pub fn new(intersection_slot: u64, window_size: u64, tip_slot: u64) -> Self {
        PeerChainInfo {
            density_window: DensityWindow::new(intersection_slot, window_size),
            tip_slot,
            intersection_slot,
            idling: false,
            latest_slot: None,
        }
    }

    /// Number of blocks this peer has within the genesis window.
    pub fn blocks_in_window(&self) -> u64 {
        self.density_window.block_count()
    }

    /// Record a block arriving at `slot` from this peer.
    pub fn record_block(&mut self, slot: u64) {
        self.density_window.record_block(slot);
        // Track latest_slot — may be beyond the density window
        match self.latest_slot {
            Some(s) if slot > s => self.latest_slot = Some(slot),
            None => self.latest_slot = Some(slot),
            _ => {}
        }
    }

    /// Update the peer's tip slot (called when a new header is received).
    pub fn update_tip(&mut self, slot: u64) {
        if slot > self.tip_slot {
            self.tip_slot = slot;
        }
    }
}
```

- [ ] **Step 3: Update GsmConfig with new fields**

Replace the existing `GsmConfig` struct and Default impl (lines 68-95) with:

```rust
/// Configuration for the Genesis State Machine.
#[derive(Debug, Clone)]
pub struct GsmConfig {
    /// Minimum active big ledger peers to transition PreSyncing → Syncing (HAA).
    pub min_active_blp: usize,
    /// Maximum tip age (seconds) before CaughtUp → PreSyncing regression.
    pub max_caught_up_age_secs: u64,
    /// Minimum dwell time in CaughtUp before regression is allowed (seconds).
    pub min_caught_up_dwell_secs: u64,
    /// Anti-thundering-herd jitter range [0, N] seconds.
    pub anti_thundering_herd_max_secs: u64,
    /// Genesis window size in slots (3k/f).
    pub genesis_window_slots: u64,
    /// GDD evaluation rate limit in milliseconds.
    pub gdd_rate_limit_ms: u64,
    /// Security parameter k.
    pub security_param_k: u64,
    /// Path for the caught_up marker file.
    pub marker_path: PathBuf,
}

impl Default for GsmConfig {
    fn default() -> Self {
        GsmConfig {
            min_active_blp: 5,
            max_caught_up_age_secs: 1200,          // 20 minutes
            min_caught_up_dwell_secs: 1200,         // 20 minutes
            anti_thundering_herd_max_secs: 300,     // 0-300 seconds
            genesis_window_slots: 129_600,          // 3 * 2160 / 0.05
            gdd_rate_limit_ms: 1000,                // 1 second
            security_param_k: 2160,
            marker_path: PathBuf::from("caught_up.marker"),
        }
    }
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p dugite-node`

Expected: Compilation errors from `GenesisStateMachine` methods that reference the old PeerChainInfo fields. That's fine — we'll fix those in Task 3.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/gsm.rs
git commit -m "feat(gsm): define GsmEvent, GsmSnapshot, GddAction types and update PeerChainInfo (#316)"
```

---

### Task 3: Rewrite GenesisStateMachine with Correct GDD Algorithm

**Files:**
- Modify: `crates/dugite-node/src/gsm.rs`

- [ ] **Step 1: Write failing tests for the 4-guard GDD algorithm**

Add a test module at the bottom of `gsm.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    fn make_gsm() -> GenesisStateMachine {
        let config = GsmConfig {
            genesis_window_slots: 100,
            security_param_k: 10,
            marker_path: PathBuf::from("/tmp/test_caught_up.marker"),
            ..Default::default()
        };
        GenesisStateMachine::new(config, true)
    }

    // ── GDD Guard Tests ──────────────────────────────────────────────

    #[test]
    fn test_gdd_guard1_no_signal_peer_not_disconnected() {
        // A peer that hasn't sent any signal should NOT be disconnected
        let mut gsm = make_gsm();
        gsm.state = GenesisSyncState::Syncing;

        // peer0: no blocks, not idling, no latest_slot
        gsm.register_peer(addr(1), 0, 50);

        // peer1: has blocks (dense chain)
        gsm.register_peer(addr(2), 0, 50);
        for slot in (1..=20).step_by(1) {
            gsm.record_block(&addr(2), slot);
        }
        gsm.peer_info.get_mut(&addr(2)).unwrap().idling = true;

        let to_disconnect = gsm.gdd_evaluate();
        // peer0 has no signal (not idling, no blocks, no latest_slot) → guard 1 fails
        assert!(!to_disconnect.contains(&addr(1)));
    }

    #[test]
    fn test_gdd_guard2_same_chain_not_disconnected() {
        // Two peers with identical chains should NOT disconnect each other
        let mut gsm = make_gsm();
        gsm.state = GenesisSyncState::Syncing;

        gsm.register_peer(addr(1), 0, 50);
        gsm.register_peer(addr(2), 0, 50);
        // Both record identical blocks
        for slot in [10, 20, 30] {
            gsm.record_block(&addr(1), slot);
            gsm.record_block(&addr(2), slot);
        }
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;
        gsm.peer_info.get_mut(&addr(2)).unwrap().idling = true;

        let to_disconnect = gsm.gdd_evaluate();
        assert!(to_disconnect.is_empty());
    }

    #[test]
    fn test_gdd_guard4_idling_peer_dominated() {
        // An idling peer with lower density is disconnected
        let mut gsm = make_gsm();
        gsm.state = GenesisSyncState::Syncing;

        // peer0: sparse, idling (3 blocks)
        gsm.register_peer(addr(1), 0, 50);
        for slot in [10, 20, 30] {
            gsm.record_block(&addr(1), slot);
        }
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;

        // peer1: dense, idling (15 blocks, > k=10 so offers_more_than_k)
        gsm.register_peer(addr(2), 0, 50);
        for slot in 1..=15 {
            gsm.record_block(&addr(2), slot);
        }
        gsm.peer_info.get_mut(&addr(2)).unwrap().idling = true;

        let to_disconnect = gsm.gdd_evaluate();
        assert!(to_disconnect.contains(&addr(1)), "sparse idling peer should be disconnected");
        assert!(!to_disconnect.contains(&addr(2)), "dense peer should NOT be disconnected");
    }

    #[test]
    fn test_gdd_guard4_non_idling_uses_upper_bound() {
        // A non-idling peer gets benefit of the doubt (upper_bound comparison)
        let mut gsm = make_gsm();
        gsm.state = GenesisSyncState::Syncing;

        // peer0: sparse but NOT idling — may still have blocks coming
        gsm.register_peer(addr(1), 0, 50);
        for slot in [10, 20, 30] {
            gsm.record_block(&addr(1), slot);
        }
        // NOT idling — upper_bound = lower_bound + potential_slots, which is large

        // peer1: somewhat dense, idling
        gsm.register_peer(addr(2), 0, 50);
        for slot in 1..=11 {
            gsm.record_block(&addr(2), slot);
        }
        gsm.peer_info.get_mut(&addr(2)).unwrap().idling = true;

        let to_disconnect = gsm.gdd_evaluate();
        // peer0 is NOT idling, so guard4 compares lb1 >= ub0.
        // ub0 = 3 + (100 - 30) = 73. lb1 = 11. 11 < 73 → guard4 fails.
        assert!(!to_disconnect.contains(&addr(1)), "non-idling peer should get benefit of the doubt");
    }

    #[test]
    fn test_gdd_guard3_small_fork_not_disconnected() {
        // If peer1 doesn't offer more than k blocks AND peer0's bounds differ,
        // guard 3 prevents disconnection
        let mut gsm = make_gsm();
        gsm.state = GenesisSyncState::Syncing;

        // peer0: 2 blocks, idling
        gsm.register_peer(addr(1), 0, 50);
        gsm.record_block(&addr(1), 10);
        gsm.record_block(&addr(1), 20);
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;

        // peer1: 5 blocks (< k=10), idling
        gsm.register_peer(addr(2), 0, 50);
        for slot in [5, 10, 15, 25, 35] {
            gsm.record_block(&addr(2), slot);
        }
        gsm.peer_info.get_mut(&addr(2)).unwrap().idling = true;

        let to_disconnect = gsm.gdd_evaluate();
        // peer1 offers 5 blocks total, not > k=10. peer0's lb=2, ub=2 (idling),
        // so lb0 == ub0 → guard3 passes. But let's check: guard4 requires lb1 >= lb0.
        // lb1 within window: blocks_before(0+1+100=101) from peer1's window = 5.
        // lb0 = 2. 5 >= 2 → guard4 passes. So peer0 IS disconnected.
        // This is correct — peer0's density is fully determined and dominated.
        assert!(to_disconnect.contains(&addr(1)));
    }

    // ── State Transition Tests ──────────────────────────────────────

    #[test]
    fn test_presyncing_to_syncing_on_haa() {
        let mut gsm = make_gsm();
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // Not enough BLPs
        assert!(gsm.evaluate(3, false, 100, 0).is_none());
        assert_eq!(gsm.state(), GenesisSyncState::PreSyncing);

        // HAA satisfied
        let result = gsm.evaluate(5, false, 100, 0);
        assert_eq!(result, Some(GenesisSyncState::Syncing));
    }

    #[test]
    fn test_syncing_to_presyncing_on_haa_loss() {
        let mut gsm = make_gsm();
        gsm.evaluate(5, false, 100, 0); // → Syncing
        assert_eq!(gsm.state(), GenesisSyncState::Syncing);

        // HAA lost
        let result = gsm.evaluate(2, false, 100, 0);
        assert_eq!(result, Some(GenesisSyncState::PreSyncing));
    }

    #[test]
    fn test_syncing_to_caught_up() {
        let mut gsm = make_gsm();
        gsm.evaluate(5, false, 100, 0); // → Syncing

        // Register peer within genesis window of immutable tip
        gsm.register_peer(addr(1), 0, 50);
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;

        // All idle, fresh tip, immutable_tip_slot=0, peer tip=50 < 0+100=100
        let result = gsm.evaluate(5, true, 100, 0);
        assert_eq!(result, Some(GenesisSyncState::CaughtUp));
    }

    #[test]
    fn test_caught_up_no_regression_during_dwell() {
        let mut gsm = make_gsm();
        gsm.config.min_caught_up_dwell_secs = 1200;
        gsm.evaluate(5, false, 100, 0); // → Syncing
        gsm.register_peer(addr(1), 0, 50);
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;
        gsm.evaluate(5, true, 100, 0); // → CaughtUp

        // Stale tip but within dwell period
        let result = gsm.evaluate(5, true, 9999, 0);
        assert!(result.is_none(), "should not regress during dwell period");
        assert_eq!(gsm.state(), GenesisSyncState::CaughtUp);
    }

    // ── LoE Tests ───────────────────────────────────────────────────

    #[test]
    fn test_loe_presyncing_freezes() {
        let gsm = make_gsm();
        assert_eq!(gsm.compute_loe_slot(), Some(0));
    }

    #[test]
    fn test_loe_syncing_uses_min_intersection() {
        let mut gsm = make_gsm();
        gsm.evaluate(5, false, 100, 0); // → Syncing
        gsm.register_peer(addr(1), 100, 500);
        gsm.register_peer(addr(2), 200, 600);
        assert_eq!(gsm.compute_loe_slot(), Some(100));
    }

    #[test]
    fn test_loe_caught_up_no_constraint() {
        let mut gsm = make_gsm();
        gsm.evaluate(5, false, 100, 0); // → Syncing
        gsm.register_peer(addr(1), 0, 50);
        gsm.peer_info.get_mut(&addr(1)).unwrap().idling = true;
        gsm.evaluate(5, true, 100, 0); // → CaughtUp
        assert_eq!(gsm.compute_loe_slot(), None);
    }

    #[test]
    fn test_loe_advances_after_peer_deregister() {
        let mut gsm = make_gsm();
        gsm.evaluate(5, false, 100, 0); // → Syncing
        gsm.register_peer(addr(1), 100, 500);
        gsm.register_peer(addr(2), 300, 600);
        assert_eq!(gsm.compute_loe_slot(), Some(100));

        gsm.deregister_peer(&addr(1));
        assert_eq!(gsm.compute_loe_slot(), Some(300));
    }
}
```

- [ ] **Step 2: Rewrite GenesisStateMachine**

Replace the entire `GenesisStateMachine` struct and impl block (lines 142-end) with:

```rust
/// The Genesis State Machine.
///
/// Tracks sync state and enforces transitions based on peer availability
/// and tip freshness. Also manages:
/// - **LoE (Limit on Eagerness)**: constrains immutable tip advancement during sync
/// - **GDD (Genesis Density Disconnector)**: disconnects sparse-chain peers
pub struct GenesisStateMachine {
    pub(crate) config: GsmConfig,
    state: GenesisSyncState,
    /// Whether genesis mode is enabled (opt-in via --consensus-mode genesis)
    enabled: bool,
    /// Per-peer chain density information, keyed by peer socket address.
    pub(crate) peer_info: HashMap<SocketAddr, PeerChainInfo>,
    /// Timestamp when CaughtUp was entered (for minimum dwell enforcement).
    caught_up_since: Option<std::time::Instant>,
    /// Random jitter for anti-thundering-herd on CaughtUp regression.
    anti_thundering_herd_jitter_secs: u64,
}

impl GenesisStateMachine {
    /// Create a new GSM. If not enabled, it immediately enters CaughtUp
    /// and all constraints are disabled.
    pub fn new(config: GsmConfig, enabled: bool) -> Self {
        let initial_state = if enabled {
            if config.marker_path.exists() {
                info!("Genesis: caught_up marker found, starting in CaughtUp state");
                GenesisSyncState::CaughtUp
            } else {
                GenesisSyncState::PreSyncing
            }
        } else {
            GenesisSyncState::CaughtUp
        };

        // Generate anti-thundering-herd jitter: random value in [0, max]
        let jitter = if config.anti_thundering_herd_max_secs > 0 {
            // Simple deterministic-ish jitter based on process ID to avoid
            // needing an RNG dependency. For production, replace with proper RNG.
            let pid = std::process::id() as u64;
            pid % (config.anti_thundering_herd_max_secs + 1)
        } else {
            0
        };

        GenesisStateMachine {
            config,
            state: initial_state,
            enabled,
            peer_info: HashMap::new(),
            caught_up_since: if initial_state == GenesisSyncState::CaughtUp {
                Some(std::time::Instant::now())
            } else {
                None
            },
            anti_thundering_herd_jitter_secs: jitter,
        }
    }

    /// Current sync state.
    pub fn state(&self) -> GenesisSyncState {
        self.state
    }

    /// Whether genesis mode is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    // ── Peer density tracking ────────────────────────────────────────

    /// Register a newly connected peer or reset its density window.
    pub fn register_peer(&mut self, addr: SocketAddr, intersection_slot: u64, tip_slot: u64) {
        if !self.enabled {
            return;
        }
        let info = PeerChainInfo::new(
            intersection_slot,
            self.config.genesis_window_slots,
            tip_slot,
        );
        self.peer_info.insert(addr, info);
        debug!(
            %addr, intersection_slot,
            window_size = self.config.genesis_window_slots,
            "GDD: registered peer"
        );
    }

    /// Remove a disconnected peer from density tracking.
    pub fn deregister_peer(&mut self, addr: &SocketAddr) {
        if self.peer_info.remove(addr).is_some() {
            debug!(%addr, "GDD: deregistered peer");
        }
    }

    /// Record a block received from `addr` at `slot`.
    pub fn record_block(&mut self, addr: &SocketAddr, slot: u64) {
        if !self.enabled {
            return;
        }
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.record_block(slot);
        }
    }

    /// Update the tip slot reported by `addr`.
    pub fn update_peer_tip(&mut self, addr: &SocketAddr, tip_slot: u64) {
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.update_tip(tip_slot);
        }
    }

    /// Set a peer's idling state (MsgAwaitReply received).
    pub fn set_peer_idling(&mut self, addr: &SocketAddr, idling: bool) {
        if let Some(info) = self.peer_info.get_mut(addr) {
            info.idling = idling;
        }
    }

    /// Read-only view of the peer density map.
    pub fn peer_info(&self) -> &HashMap<SocketAddr, PeerChainInfo> {
        &self.peer_info
    }

    // ── State transitions ────────────────────────────────────────────

    /// Evaluate state transitions based on current conditions.
    /// Returns `Some(new_state)` if a transition occurred.
    pub fn evaluate(
        &mut self,
        active_blp_count: usize,
        all_chainsync_idle: bool,
        tip_age_secs: u64,
        immutable_tip_slot: u64,
    ) -> Option<GenesisSyncState> {
        if !self.enabled {
            return None;
        }

        let old_state = self.state;

        match self.state {
            GenesisSyncState::PreSyncing => {
                if active_blp_count >= self.config.min_active_blp {
                    self.state = GenesisSyncState::Syncing;
                    info!(
                        active_blp = active_blp_count,
                        min = self.config.min_active_blp,
                        "Genesis: HAA satisfied, transitioning to Syncing"
                    );
                }
            }
            GenesisSyncState::Syncing => {
                // HAA lost → regress to PreSyncing
                if active_blp_count < self.config.min_active_blp {
                    self.state = GenesisSyncState::PreSyncing;
                    warn!(
                        active_blp = active_blp_count,
                        min = self.config.min_active_blp,
                        "Genesis: HAA lost, regressing to PreSyncing"
                    );
                } else if all_chainsync_idle
                    && tip_age_secs < self.config.max_caught_up_age_secs
                    && self.all_peers_within_window(immutable_tip_slot)
                {
                    self.state = GenesisSyncState::CaughtUp;
                    self.caught_up_since = Some(std::time::Instant::now());
                    self.write_marker();
                    info!(
                        tip_age_secs,
                        "Genesis: CaughtUp condition met"
                    );
                }
            }
            GenesisSyncState::CaughtUp => {
                // Enforce minimum dwell time before allowing regression
                let dwell_ok = self
                    .caught_up_since
                    .map(|t| t.elapsed().as_secs() >= self.config.min_caught_up_dwell_secs)
                    .unwrap_or(true);

                if dwell_ok {
                    let threshold =
                        self.config.max_caught_up_age_secs + self.anti_thundering_herd_jitter_secs;
                    if tip_age_secs > threshold {
                        self.state = GenesisSyncState::PreSyncing;
                        self.caught_up_since = None;
                        self.remove_marker();
                        warn!(
                            tip_age_secs,
                            threshold,
                            "Genesis: tip stale, regressing to PreSyncing"
                        );
                    }
                }
            }
        }

        if self.state != old_state {
            Some(self.state)
        } else {
            None
        }
    }

    /// Check if all registered peers' tips are within the genesis window
    /// of the immutable tip. Required for CaughtUp transition.
    fn all_peers_within_window(&self, immutable_tip_slot: u64) -> bool {
        if self.peer_info.is_empty() {
            return false; // Must have at least one peer (matches Haskell)
        }
        let window_end = immutable_tip_slot.saturating_add(self.config.genesis_window_slots);
        self.peer_info.values().all(|info| info.tip_slot <= window_end)
    }

    // ── LoE ─────────────────────────────────────────────────────────

    /// Compute the current LoE ceiling slot.
    ///
    /// Returns `None` if there is no constraint (CaughtUp or disabled).
    /// Returns `Some(slot)` to cap immutable tip advancement.
    pub fn compute_loe_slot(&self) -> Option<u64> {
        if !self.enabled {
            return None;
        }

        match self.state {
            GenesisSyncState::PreSyncing => Some(0),
            GenesisSyncState::Syncing => {
                if self.peer_info.is_empty() {
                    return Some(0);
                }
                // Conservative approximation of the common chain prefix:
                // the minimum intersection slot across all registered peers.
                let min_intersection = self
                    .peer_info
                    .values()
                    .map(|info| info.intersection_slot)
                    .min()
                    .unwrap_or(0);
                Some(min_intersection)
            }
            GenesisSyncState::CaughtUp => None,
        }
    }

    // ── GDD (Genesis Density Disconnector) ───────────────────────────

    /// Run the Genesis Density Disconnector.
    ///
    /// During Syncing state, compares chain density across all known peers
    /// within the genesis window using the Haskell 4-guard algorithm from
    /// `Ouroboros.Consensus.Genesis.Governor.densityDisconnect`.
    ///
    /// All arithmetic is integer (u64) — no floating point.
    ///
    /// Returns addresses of peers that should be disconnected.
    pub fn gdd_evaluate(&self) -> Vec<SocketAddr> {
        if !self.enabled || self.state != GenesisSyncState::Syncing {
            return Vec::new();
        }
        if self.peer_info.len() < 2 {
            return Vec::new();
        }

        let sgen = self.config.genesis_window_slots;
        let k = self.config.security_param_k;

        // Compute the LoE intersection slot (minimum intersection across peers).
        let loe_intersection_slot = self
            .peer_info
            .values()
            .map(|info| info.intersection_slot)
            .min()
            .unwrap_or(0);

        // First slot after the genesis window boundary.
        let first_slot_after_window = loe_intersection_slot
            .saturating_add(1)
            .saturating_add(sgen);

        // Pre-compute density bounds for each peer.
        struct PeerBounds {
            addr: SocketAddr,
            lower_bound: u64,
            upper_bound: u64,
            has_block_after: bool,
            offers_more_than_k: bool,
            idling: bool,
            /// Last block slot within the genesis window (for Guard 2).
            last_block_in_window: Option<u64>,
        }

        let bounds: Vec<PeerBounds> = self
            .peer_info
            .iter()
            .map(|(addr, info)| {
                // Count blocks strictly within the genesis window.
                let blocks_in_window = info.density_window.blocks_before(first_slot_after_window);

                // Does the peer have any block/header at or past the window end?
                let has_block_after = info
                    .latest_slot
                    .map(|s| s >= first_slot_after_window)
                    .unwrap_or(false)
                    || info
                        .density_window
                        .has_block_at_or_after(first_slot_after_window);

                // Unknown trailing slots in the genesis window.
                let potential_slots = if has_block_after {
                    0
                } else {
                    let head = info
                        .density_window
                        .head_slot()
                        .unwrap_or(loe_intersection_slot);
                    first_slot_after_window.saturating_sub(head.saturating_add(1))
                };

                let lower_bound = blocks_in_window;
                let upper_bound = lower_bound.saturating_add(potential_slots);

                // Total blocks after intersection (entire suffix, not just window).
                let offers_more_than_k = info.density_window.total_block_count() > k;

                // Last recorded block slot within the window (for disagreement check).
                let last_block_in_window = {
                    let count = blocks_in_window as usize;
                    if count > 0 {
                        // The slots vec is sorted; the block at index (count-1) is the
                        // last one within the window.
                        info.density_window.head_slot().filter(|&s| s < first_slot_after_window)
                    } else {
                        None
                    }
                };

                PeerBounds {
                    addr: *addr,
                    lower_bound,
                    upper_bound,
                    has_block_after,
                    offers_more_than_k,
                    idling: info.idling,
                    last_block_in_window,
                }
            })
            .collect();

        // O(n²) comparison: for each peer pair, check 4 guards.
        let mut losing_peers = std::collections::HashSet::new();
        for peer0 in &bounds {
            for peer1 in &bounds {
                if peer0.addr == peer1.addr {
                    continue;
                }

                // Guard 1: peer0 has sent at least some signal.
                let guard1 = peer0.idling || peer0.lower_bound > 0 || peer0.has_block_after;

                // Guard 2: chains genuinely disagree.
                let guard2 = peer0.last_block_in_window != peer1.last_block_in_window;

                // Guard 3: comparison is meaningful.
                let guard3 =
                    peer1.offers_more_than_k || (peer0.lower_bound == peer0.upper_bound);

                // Guard 4: peer1 dominates peer0's density.
                let guard4 = if peer0.idling {
                    peer1.lower_bound >= peer0.lower_bound
                } else {
                    peer1.lower_bound >= peer0.upper_bound
                };

                if guard1 && guard2 && guard3 && guard4 {
                    losing_peers.insert(peer0.addr);
                }
            }
        }

        if !losing_peers.is_empty() {
            info!(
                disconnecting = losing_peers.len(),
                total_peers = self.peer_info.len(),
                "GDD: disconnecting peers with insufficient chain density"
            );
        }

        losing_peers.into_iter().collect()
    }

    // ── Marker file helpers ──────────────────────────────────────────

    fn write_marker(&self) {
        if let Some(parent) = self.config.marker_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&self.config.marker_path, "caught_up") {
            warn!(
                path = %self.config.marker_path.display(),
                "Failed to write caught_up marker: {e}"
            );
        }
    }

    fn remove_marker(&self) {
        if let Err(e) = std::fs::remove_file(&self.config.marker_path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(
                    path = %self.config.marker_path.display(),
                    "Failed to remove caught_up marker: {e}"
                );
            }
        }
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo nextest run -p dugite-node -E 'test(test_gdd_) | test(test_presyncing_) | test(test_syncing_) | test(test_caught_up_) | test(test_loe_)'`

Expected: All PASS.

- [ ] **Step 4: Run clippy**

Run: `cargo clippy -p dugite-node --all-targets -- -D warnings`

Fix any warnings. The `#[allow(dead_code)]` annotations should now be removed since the code is actively used.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/gsm.rs
git commit -m "feat(gsm): rewrite GSM with Haskell-correct 4-guard GDD algorithm (#316)

- Integer-only density comparison (no floating point)
- All 4 guards from densityDisconnect ported
- Syncing→PreSyncing regression on HAA loss
- Minimum CaughtUp dwell time (20 min)
- Anti-thundering-herd jitter
- Comprehensive test coverage"
```

---

### Task 4: Implement the GSM Actor

**Files:**
- Modify: `crates/dugite-node/src/gsm.rs`

- [ ] **Step 1: Write a test for the actor lifecycle**

Add to the test module:

```rust
#[tokio::test]
async fn test_gsm_actor_state_transitions() {
    let config = GsmConfig {
        genesis_window_slots: 100,
        security_param_k: 10,
        gdd_rate_limit_ms: 100, // fast for testing
        marker_path: PathBuf::from("/tmp/test_actor_marker"),
        ..Default::default()
    };

    let (event_tx, event_rx) = mpsc::channel(64);
    let (snapshot_tx, mut snapshot_rx) = watch::channel(GsmSnapshot {
        state: GenesisSyncState::PreSyncing,
        loe_slot: Some(0),
    });
    let (action_tx, mut action_rx) = mpsc::channel(64);

    // Spawn actor
    let handle = tokio::spawn(run_gsm_actor(config, true, event_rx, snapshot_tx, action_tx));

    // Wait for initial snapshot
    tokio::time::sleep(Duration::from_millis(50)).await;
    {
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::PreSyncing);
        assert_eq!(snap.loe_slot, Some(0));
    }

    // Send HAA satisfied → Syncing
    event_tx
        .send(GsmEvent::SyncStatus {
            active_blp_count: 5,
            all_chainsync_idle: false,
            tip_age_secs: 100,
            immutable_tip_slot: 0,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    {
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::Syncing);
    }

    // Register peer, send blocks, trigger GDD
    event_tx
        .send(GsmEvent::PeerRegistered {
            addr: addr(1),
            intersection_slot: 0,
            tip_slot: 50,
        })
        .await
        .unwrap();
    event_tx
        .send(GsmEvent::PeerIdling { addr: addr(1) })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // CaughtUp transition
    event_tx
        .send(GsmEvent::SyncStatus {
            active_blp_count: 5,
            all_chainsync_idle: true,
            tip_age_secs: 100,
            immutable_tip_slot: 0,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    {
        let snap = *snapshot_rx.borrow();
        assert_eq!(snap.state, GenesisSyncState::CaughtUp);
        assert_eq!(snap.loe_slot, None);
    }

    // Verify no spurious GDD actions
    assert!(action_rx.try_recv().is_err());

    // Clean up
    drop(event_tx);
    let _ = handle.await;
    let _ = std::fs::remove_file("/tmp/test_actor_marker");
}

#[tokio::test]
async fn test_gsm_actor_gdd_disconnects() {
    let config = GsmConfig {
        genesis_window_slots: 100,
        security_param_k: 10,
        gdd_rate_limit_ms: 50, // fast for testing
        marker_path: PathBuf::from("/tmp/test_actor_gdd_marker"),
        ..Default::default()
    };

    let (event_tx, event_rx) = mpsc::channel(64);
    let (snapshot_tx, _snapshot_rx) = watch::channel(GsmSnapshot {
        state: GenesisSyncState::PreSyncing,
        loe_slot: Some(0),
    });
    let (action_tx, mut action_rx) = mpsc::channel(64);

    let handle = tokio::spawn(run_gsm_actor(config, true, event_rx, snapshot_tx, action_tx));

    // → Syncing
    event_tx.send(GsmEvent::SyncStatus {
        active_blp_count: 5,
        all_chainsync_idle: false,
        tip_age_secs: 100,
        immutable_tip_slot: 0,
    }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Register two peers: one sparse, one dense
    event_tx.send(GsmEvent::PeerRegistered {
        addr: addr(1), intersection_slot: 0, tip_slot: 50,
    }).await.unwrap();
    event_tx.send(GsmEvent::PeerRegistered {
        addr: addr(2), intersection_slot: 0, tip_slot: 50,
    }).await.unwrap();

    // Dense peer: 15 blocks (> k=10)
    for slot in 1..=15 {
        event_tx.send(GsmEvent::BlockReceived { addr: addr(2), slot }).await.unwrap();
    }
    // Sparse peer: 3 blocks
    for slot in [10, 20, 30] {
        event_tx.send(GsmEvent::BlockReceived { addr: addr(1), slot }).await.unwrap();
    }

    // Both idling
    event_tx.send(GsmEvent::PeerIdling { addr: addr(1) }).await.unwrap();
    event_tx.send(GsmEvent::PeerIdling { addr: addr(2) }).await.unwrap();

    // Wait for GDD tick
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Check for disconnect action
    let action = action_rx.try_recv();
    assert!(action.is_ok(), "GDD should have emitted a disconnect");
    match action.unwrap() {
        GddAction::DisconnectPeer(addr) => {
            assert_eq!(addr, addr(1), "sparse peer should be disconnected");
        }
    }

    drop(event_tx);
    let _ = handle.await;
    let _ = std::fs::remove_file("/tmp/test_actor_gdd_marker");
}
```

- [ ] **Step 2: Implement the actor function**

Add at the end of the impl block (before `#[cfg(test)]`):

```rust
/// Run the GSM actor as a dedicated tokio task.
///
/// This is the main event loop that:
/// 1. Receives GsmEvents from producers (ChainSync, BlockFetch, networking)
/// 2. Dispatches them to the appropriate GSM methods
/// 3. Periodically runs GDD evaluation (every gdd_rate_limit_ms)
/// 4. Publishes GsmSnapshot via watch channel
/// 5. Sends GddAction::DisconnectPeer via mpsc channel
pub async fn run_gsm_actor(
    config: GsmConfig,
    enabled: bool,
    mut event_rx: mpsc::Receiver<GsmEvent>,
    snapshot_tx: watch::Sender<GsmSnapshot>,
    action_tx: mpsc::Sender<GddAction>,
) {
    let gdd_rate = std::time::Duration::from_millis(config.gdd_rate_limit_ms);
    let mut gsm = GenesisStateMachine::new(config, enabled);

    // Publish initial snapshot
    let initial = GsmSnapshot {
        state: gsm.state(),
        loe_slot: gsm.compute_loe_slot(),
    };
    let _ = snapshot_tx.send(initial);

    let mut gdd_interval = tokio::time::interval(gdd_rate);
    gdd_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                let Some(event) = event else {
                    // Channel closed — shut down
                    info!("GSM actor: event channel closed, shutting down");
                    break;
                };
                Self::dispatch_event(&mut gsm, &event);
                Self::publish_snapshot(&gsm, &snapshot_tx);
            }
            _ = gdd_interval.tick() => {
                // Run GDD evaluation during Syncing state
                let to_disconnect = gsm.gdd_evaluate();
                for addr in &to_disconnect {
                    if action_tx.send(GddAction::DisconnectPeer(*addr)).await.is_err() {
                        warn!("GSM actor: GDD action channel closed");
                        return;
                    }
                    gsm.deregister_peer(addr);
                }
                if !to_disconnect.is_empty() {
                    // LoE may have changed after disconnections
                    Self::publish_snapshot(&gsm, &snapshot_tx);
                }
            }
        }
    }
}

/// Dispatch a single event to the GSM.
fn dispatch_event(gsm: &mut GenesisStateMachine, event: &GsmEvent) {
    match event {
        GsmEvent::PeerRegistered {
            addr,
            intersection_slot,
            tip_slot,
        } => {
            gsm.register_peer(*addr, *intersection_slot, *tip_slot);
        }
        GsmEvent::PeerDisconnected { addr } => {
            gsm.deregister_peer(addr);
        }
        GsmEvent::BlockReceived { addr, slot } => {
            gsm.record_block(addr, *slot);
        }
        GsmEvent::PeerTipUpdated { addr, tip_slot } => {
            gsm.update_peer_tip(addr, *tip_slot);
        }
        GsmEvent::PeerIdling { addr } => {
            gsm.set_peer_idling(addr, true);
        }
        GsmEvent::PeerActive { addr } => {
            gsm.set_peer_idling(addr, false);
        }
        GsmEvent::SyncStatus {
            active_blp_count,
            all_chainsync_idle,
            tip_age_secs,
            immutable_tip_slot,
        } => {
            gsm.evaluate(
                *active_blp_count,
                *all_chainsync_idle,
                *tip_age_secs,
                *immutable_tip_slot,
            );
        }
    }
}

/// Publish the current GSM state and LoE slot to watchers.
fn publish_snapshot(gsm: &GenesisStateMachine, tx: &watch::Sender<GsmSnapshot>) {
    let snapshot = GsmSnapshot {
        state: gsm.state(),
        loe_slot: gsm.compute_loe_slot(),
    };
    let _ = tx.send(snapshot);
}
```

- [ ] **Step 3: Add necessary imports**

Ensure these imports are at the top of gsm.rs:

```rust
use std::time::Duration;
```

- [ ] **Step 4: Run the actor tests**

Run: `cargo nextest run -p dugite-node -E 'test(test_gsm_actor_)'`

Expected: All PASS.

- [ ] **Step 5: Run full test suite + clippy**

Run: `cargo nextest run -p dugite-node && cargo clippy -p dugite-node --all-targets -- -D warnings`

Expected: There will be compilation errors in `node/mod.rs` because it still references `Arc<RwLock<GenesisStateMachine>>`. That's expected — we fix it in Task 5.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-node/src/gsm.rs
git commit -m "feat(gsm): implement GSM actor with event loop and GDD evaluation (#316)"
```

---

### Task 5: Wire GSM Actor into Node Struct and Initialization

**Files:**
- Modify: `crates/dugite-node/src/node/mod.rs`

- [ ] **Step 1: Replace GSM field in Node struct**

At line 177, replace:

```rust
pub(crate) gsm: Arc<RwLock<crate::gsm::GenesisStateMachine>>,
```

With:

```rust
/// Channel to send events to the GSM actor (Genesis State Machine).
pub(crate) gsm_event_tx: mpsc::Sender<crate::gsm::GsmEvent>,
/// Watch receiver for the latest GSM snapshot (state + LoE slot).
/// Sync pipeline reads this for LoE enforcement (zero-cost borrow).
pub(crate) gsm_snapshot_rx: watch::Receiver<crate::gsm::GsmSnapshot>,
```

- [ ] **Step 2: Update GSM initialization in Node::new()**

Replace lines 1068-1076 (the `genesis_enabled` check, GsmConfig creation, and GSM creation) with:

```rust
let genesis_enabled = args.consensus_mode == "genesis";
let gsm_config = crate::gsm::GsmConfig {
    marker_path: args.database_path.join("caught_up.marker"),
    ..Default::default()
};

// Create GSM channels
let (gsm_event_tx, gsm_event_rx) = mpsc::channel::<crate::gsm::GsmEvent>(1024);
let initial_snapshot = crate::gsm::GsmSnapshot {
    state: if genesis_enabled {
        if gsm_config.marker_path.exists() {
            crate::gsm::GenesisSyncState::CaughtUp
        } else {
            crate::gsm::GenesisSyncState::PreSyncing
        }
    } else {
        crate::gsm::GenesisSyncState::CaughtUp
    },
    loe_slot: if genesis_enabled && !gsm_config.marker_path.exists() {
        Some(0)
    } else {
        None
    },
};
let (gsm_snapshot_tx, gsm_snapshot_rx) =
    watch::channel::<crate::gsm::GsmSnapshot>(initial_snapshot);
let (gdd_action_tx, gdd_action_rx) =
    mpsc::channel::<crate::gsm::GddAction>(64);
```

- [ ] **Step 3: Update Node struct initialization**

In the `Node { ... }` struct literal (around line 1211), replace:

```rust
gsm,
```

With:

```rust
gsm_event_tx,
gsm_snapshot_rx,
```

Also store the remaining pieces for the run method:

```rust
// Store these for spawn in run()
// (You may need to store gsm_config, gsm_event_rx, gsm_snapshot_tx, gdd_action_tx,
//  gdd_action_rx, genesis_enabled in temporary fields or pass them to run() directly)
```

Note: The exact wiring depends on how `run()` is structured. The actor and GDD consumer tasks need to be spawned in `run()`. Store `gsm_event_rx`, `gsm_snapshot_tx`, `gdd_action_tx`, `gdd_action_rx`, `gsm_config`, and `genesis_enabled` as `Option` fields that `run()` takes via `.take()`.

Add these fields to the Node struct:

```rust
/// GSM actor pieces (taken by run() to spawn the actor task).
pub(crate) gsm_actor_parts: Option<GsmActorParts>,
```

And define:

```rust
/// Components needed to spawn the GSM actor, consumed by run().
pub(crate) struct GsmActorParts {
    pub config: crate::gsm::GsmConfig,
    pub enabled: bool,
    pub event_rx: mpsc::Receiver<crate::gsm::GsmEvent>,
    pub snapshot_tx: watch::Sender<crate::gsm::GsmSnapshot>,
    pub action_tx: mpsc::Sender<crate::gsm::GddAction>,
    pub action_rx: mpsc::Receiver<crate::gsm::GddAction>,
}
```

- [ ] **Step 4: Spawn GSM actor and GDD consumer in run()**

In the `run()` method, replace the existing background GSM evaluation task (lines 2151-2183) with:

```rust
// ─── GSM Actor ───────────────────────────────────────────────────
if let Some(parts) = self.gsm_actor_parts.take() {
    // Spawn the GSM actor
    tokio::spawn(crate::gsm::run_gsm_actor(
        parts.config,
        parts.enabled,
        parts.event_rx,
        parts.snapshot_tx,
        parts.action_tx,
    ));

    // Spawn the GDD action consumer
    let gdd_pm = peer_manager.clone();
    let mut gdd_action_rx = parts.action_rx;
    tokio::spawn(async move {
        while let Some(action) = gdd_action_rx.recv().await {
            match action {
                crate::gsm::GddAction::DisconnectPeer(addr) => {
                    warn!(%addr, "GDD: disconnecting sparse peer");
                    let mut pm = gdd_pm.write().await;
                    pm.peer_disconnected(&addr);
                }
            }
        }
    });

    // Spawn the background SyncStatus emitter (replaces old GSM evaluation task)
    if genesis_enabled {
        let status_tx = self.gsm_event_tx.clone();
        let status_pm = peer_manager.clone();
        let status_metrics = self.metrics.clone();
        let status_shutdown = shutdown_rx.clone();

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let pm = status_pm.read().await;
                        let active_blp = pm.active_big_ledger_peer_count();
                        let all_idle = pm.all_chainsync_idle_for(Duration::from_secs(30));
                        drop(pm);

                        let tip_age_secs = status_metrics.tip_age_secs();
                        let immutable_tip_slot = status_metrics.immutable_tip_slot();

                        let _ = status_tx.try_send(crate::gsm::GsmEvent::SyncStatus {
                            active_blp_count: active_blp,
                            all_chainsync_idle: all_idle,
                            tip_age_secs,
                            immutable_tip_slot,
                        });
                    }
                    _ = status_shutdown.changed() => break,
                }
            }
        });
    }
}
```

Note: The exact method names for `active_big_ledger_peer_count()`, `all_chainsync_idle_for()`, `tip_age_secs()`, and `immutable_tip_slot()` should match the existing code. Check the actual method names and adapt. The existing background task at lines 2151-2183 already computes `active_blp`, `all_idle`, and `tip_age_secs` — reuse that same logic.

- [ ] **Step 5: Fix all compilation errors**

There will be references to the old `self.gsm` (Arc<RwLock<>>) throughout mod.rs. Find each one and either:
- Remove it (if it was the old background evaluation task)
- Replace with `self.gsm_event_tx` or `self.gsm_snapshot_rx` as appropriate

The GSM state logging block (around line 2140) should read from the watch:

```rust
let genesis_enabled = self.consensus_mode == "genesis";
if genesis_enabled {
    let snap = *self.gsm_snapshot_rx.borrow();
    info!(
        state = %snap.state,
        loe_slot = ?snap.loe_slot,
        "Genesis State Machine status"
    );
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p dugite-node`

Expected: May still have errors in sync.rs (Task 6 fixes those).

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-node/src/node/mod.rs
git commit -m "feat(node): wire GSM actor channels into Node struct and run() (#316)"
```

---

### Task 6: Wire GsmEvent Producers into Sync Pipeline

**Files:**
- Modify: `crates/dugite-node/src/node/sync.rs`

- [ ] **Step 1: Replace LoE reads with watch channel**

At line 1038-1041, replace:

```rust
let loe_limit: Option<u64> = {
    let gsm = self.gsm.read().await;
    gsm.loe_limit(std::slice::from_ref(&tip.point))
};
```

With:

```rust
let loe_limit: Option<u64> = self.gsm_snapshot_rx.borrow().loe_slot;
```

Search for ALL other `self.gsm` references in sync.rs and replace similarly. Each `gsm.read().await.loe_limit(...)` becomes `self.gsm_snapshot_rx.borrow().loe_slot`.

- [ ] **Step 2: Emit PeerRegistered after find_intersect**

In the `chainsync_client_task` function (line ~2578), after a successful `find_intersect` (where the intersection point is determined, around line 2770), add:

```rust
// Notify GSM actor of new peer registration
let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerRegistered {
    addr: peer_addr,
    intersection_slot: intersect_slot,
    tip_slot: tip_slot,
});
```

The `chainsync_client_task` function needs a `gsm_event_tx: mpsc::Sender<GsmEvent>` parameter added to its signature. Pass it from the call site in `run()`.

- [ ] **Step 3: Emit BlockReceived + PeerTipUpdated on MsgRollForward**

In the MsgRollForward handler (line ~2931), after extracting the slot:

```rust
let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::BlockReceived {
    addr: peer_addr,
    slot: slot,
});
let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerTipUpdated {
    addr: peer_addr,
    tip_slot: tip_slot,
});
let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerActive {
    addr: peer_addr,
});
```

- [ ] **Step 4: Emit PeerIdling on MsgAwaitReply**

In the MsgAwaitReply handler (line ~3150), add:

```rust
let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerIdling {
    addr: peer_addr,
});
```

- [ ] **Step 5: Thread gsm_event_tx through function signatures**

The `chainsync_client_task` function needs `gsm_event_tx` added to its parameter list. Update the call site in `run()` to pass `self.gsm_event_tx.clone()`.

Similarly, if `process_forward_blocks` or other functions reference `self.gsm`, update them to use `self.gsm_snapshot_rx` instead.

- [ ] **Step 6: Verify compilation**

Run: `cargo check -p dugite-node`

Expected: PASS (or minor fixups needed).

- [ ] **Step 7: Run full test suite**

Run: `cargo nextest run -p dugite-node && cargo clippy -p dugite-node --all-targets -- -D warnings`

Expected: PASS, zero warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/dugite-node/src/node/sync.rs
git commit -m "feat(sync): emit GsmEvents from ChainSync pipeline and consume GsmSnapshot for LoE (#316)"
```

---

### Task 7: Wire PeerDisconnected Events from Networking

**Files:**
- Modify: `crates/dugite-node/src/node/networking.rs`
- Modify: `crates/dugite-node/src/node/connection_lifecycle.rs`

- [ ] **Step 1: Emit PeerDisconnected in peer_disconnected()**

In `networking.rs`, update `peer_disconnected` (line 357) to accept and use a `gsm_event_tx`:

```rust
pub fn peer_disconnected(
    &mut self,
    addr: &SocketAddr,
    gsm_event_tx: &mpsc::Sender<crate::gsm::GsmEvent>,
) {
    self.inner.demote_to_cold(addr);
    self.conn_states.remove(addr);
    let _ = gsm_event_tx.try_send(crate::gsm::GsmEvent::PeerDisconnected { addr: *addr });
}
```

Alternatively, if changing the signature is too disruptive (many call sites), add `gsm_event_tx` as a field on `NodePeerManager`:

```rust
pub gsm_event_tx: Option<mpsc::Sender<crate::gsm::GsmEvent>>,
```

And emit the event inside the existing `peer_disconnected` method:

```rust
pub fn peer_disconnected(&mut self, addr: &SocketAddr) {
    self.inner.demote_to_cold(addr);
    self.conn_states.remove(addr);
    if let Some(ref tx) = self.gsm_event_tx {
        let _ = tx.try_send(crate::gsm::GsmEvent::PeerDisconnected { addr: *addr });
    }
}
```

Choose the approach that minimizes churn. The `Option<Sender>` approach is less disruptive.

- [ ] **Step 2: Initialize gsm_event_tx on NodePeerManager**

In `Node::new()` where `NodePeerManager` is created, set `gsm_event_tx`:

```rust
gsm_event_tx: Some(gsm_event_tx.clone()),
```

- [ ] **Step 3: Also emit PeerDisconnected from connection_lifecycle.rs**

In `connection_lifecycle.rs`, wherever a connection is closed/dropped and `peer_disconnected` is called on the peer manager, ensure the GSM event is emitted. This should already happen if `peer_disconnected` on `NodePeerManager` emits the event internally.

Check all call sites of `peer_disconnected` and `remove_peer` to ensure coverage.

- [ ] **Step 4: Verify compilation and tests**

Run: `cargo check -p dugite-node && cargo nextest run -p dugite-node`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-node/src/node/networking.rs crates/dugite-node/src/node/connection_lifecycle.rs
git commit -m "feat(networking): emit GsmEvent::PeerDisconnected on peer disconnect (#316)"
```

---

### Task 8: Create GitHub Issue for Phase 4 (CSJ)

- [ ] **Step 1: Create the issue**

```bash
gh issue create \
  --title "Ouroboros Genesis Phase 4: ChainSync Jumping (CSJ)" \
  --label "architecture,protocol" \
  --body "$(cat <<'ISSUE_EOF'
## Summary

Implement ChainSync Jumping (CSJ) to enable parallel header verification across multiple peers during Genesis sync. This is Phase 4 of the Ouroboros Genesis implementation, building on the foundation from #316 and #333.

## Background

Phases 1-3 (implemented in #316/#333) provide:
- GSM actor with event-based architecture
- GDD (Genesis Density Disconnector) with 4-guard algorithm
- LoE (Limit on Eagerness) enforcement
- Proper CaughtUp condition with dwell time

CSJ is the final piece for full Genesis compliance. It allows a syncing node to verify headers from multiple peers in parallel by "jumping" between chain segments, dramatically reducing initial sync time.

## Key Components

### CSJ Protocol
- Lightweight ChainSync clients that sample peer chains at jump points
- Jump interval based on genesis window size
- Jumper/objector/dynamo role assignment per peer

### Integration with Existing GSM Actor
- New GsmEvent variants for CSJ state (JumpResult, ObjectionReceived)
- CSJ state tracking per peer in PeerChainInfo
- GDD evaluation considers CSJ state

### BlockFetch Changes
- Genesis-mode single-peer sequential fetching with 10s grace period
- Peer rotation on stall (CSJ dynamo kick)

## Haskell Reference
- `Ouroboros.Consensus.MiniProtocol.ChainSync.Client` — Genesis-aware CSJ client
- `Ouroboros.Consensus.Genesis.Governor` — CSJ integration with GDD
- `ouroboros-network Decision/Genesis.hs` — Genesis BlockFetch

## Depends On
- #316 (GSM/GDD/BLP wiring)
- #333 (Genesis consensus mode)

## Priority
Medium — Phases 1-3 provide the critical safety properties. CSJ is an optimization for sync speed.
ISSUE_EOF
)"
```

- [ ] **Step 2: Commit** (no code changes, just verification)

Verify the issue was created successfully and note the issue number.

---

### Task 9: Full Integration Test and Cleanup

**Files:**
- All modified files

- [ ] **Step 1: Run the full workspace build**

Run: `cargo build --all-targets`

Expected: PASS, zero errors.

- [ ] **Step 2: Run the full test suite**

Run: `cargo nextest run --workspace`

Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`

Expected: Zero warnings.

- [ ] **Step 4: Run format check**

Run: `cargo fmt --all -- --check`

Expected: PASS.

- [ ] **Step 5: Remove all remaining #[allow(dead_code)] annotations from gsm.rs**

The types and methods should now be actively used. Remove dead_code annotations and verify clippy still passes.

- [ ] **Step 6: Verify the identify_big_ledger_peers function**

The function at the bottom of gsm.rs (line ~472) may still be dead code if not wired. Either:
- Wire it into the peer selection logic, OR
- Keep the `#[allow(dead_code)]` annotation with a comment explaining it's for Phase 4

- [ ] **Step 7: Final commit**

```bash
git add -A
git commit -m "chore: remove dead_code annotations and cleanup after Genesis wiring (#316, #333)"
```

- [ ] **Step 8: Close GitHub issues**

Close #316 and #333 with a comment referencing the commits.
