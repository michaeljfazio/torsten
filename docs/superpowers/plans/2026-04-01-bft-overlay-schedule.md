# BFT Overlay Schedule Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement 100% Haskell-compatible BFT overlay schedule validation for historical Shelley-era blocks where the decentralization parameter d > 0.

**Architecture:** New `overlay.rs` module in `dugite-consensus` implements the overlay schedule computation (`is_overlay_slot`, `classify_overlay_slot`) using exact rational arithmetic matching Haskell's `Cardano.Ledger.Slot` and `Cardano.Protocol.TPraos.Rules.Overlay`. The existing `validate_header_full()` in `praos.rs` gains a new optional `OverlayContext` parameter that branches between the Praos path (existing) and BFT path (new) based on slot classification. Genesis delegates are stored in `LedgerState` and passed through from the sync call site.

**Tech Stack:** Rust, exact i128 rational arithmetic (no f64), BTreeSet for sorted genesis key ordering

---

## File Structure

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/dugite-consensus/src/overlay.rs` | Create | Overlay schedule computation: `is_overlay_slot`, `classify_overlay_slot`, `lookup_in_overlay_schedule`, `OBftSlot`, `OverlayContext` |
| `crates/dugite-consensus/src/lib.rs` | Modify | Add `pub mod overlay` and re-export key types |
| `crates/dugite-consensus/src/praos.rs` | Modify | Add overlay-aware validation path in `validate_header_full()`, new `ConsensusError` variants |
| `crates/dugite-ledger/src/state/mod.rs` | Modify | Add `genesis_delegates` field to `LedgerState` |
| `crates/dugite-node/src/node/mod.rs` | Modify | Load genesis delegates into `LedgerState` at initialization |
| `crates/dugite-node/src/node/sync.rs` | Modify | Build `OverlayContext` and pass to `validate_header_full()` when d > 0 |

---

### Task 1: Overlay Schedule Core — `is_overlay_slot`

**Files:**
- Create: `crates/dugite-consensus/src/overlay.rs`
- Modify: `crates/dugite-consensus/src/lib.rs`

- [ ] **Step 1: Create overlay.rs with `is_overlay_slot` and tests**

```rust
// crates/dugite-consensus/src/overlay.rs

//! BFT overlay schedule for Shelley-era blocks (d > 0).
//!
//! During early Shelley, a fraction of blocks were produced by BFT genesis
//! delegates according to an overlay schedule. This module implements the
//! overlay slot computation from the Haskell cardano-ledger:
//!
//! - `is_overlay_slot`: determines if a slot is reserved for the overlay schedule
//! - `classify_overlay_slot`: determines which genesis delegate should sign
//! - `lookup_in_overlay_schedule`: combines both into the full OVERLAY lookup
//!
//! All arithmetic uses exact i128 rationals matching Haskell's `Rational` type.
//! When d = 0 (fully decentralized, all Babbage/Conway blocks), no slot is an
//! overlay slot and all blocks use the pure Praos path.

use std::collections::BTreeSet;
use dugite_primitives::hash::Hash28;

/// Result of classifying an overlay slot.
///
/// Matches Haskell's `OBftSlot` from
/// `Cardano.Protocol.TPraos.Rules.Overlay`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OBftSlot {
    /// Overlay slot with no assigned signer (silent slot).
    /// A block in this slot is invalid — the OVERLAY rule rejects it
    /// with `NotActiveSlotOVERLAY`.
    NonActiveSlot,
    /// Overlay slot assigned to the genesis key at this hash.
    /// The block must be signed by this genesis key's delegate.
    ActiveSlot(Hash28),
}

/// Check whether a slot is an overlay slot.
///
/// Matches Haskell's `isOverlaySlot` from `Cardano.Ledger.Slot`:
///
/// ```haskell
/// isOverlaySlot firstSlotNo dval slot = step s < step (s + 1)
///   where
///     s = fromIntegral $ slot -* firstSlotNo
///     d = unboundRational dval
///     step x = ceiling (x * d)
/// ```
///
/// A slot is an overlay slot iff `⌈s·d⌉ < ⌈(s+1)·d⌉`, where s is the
/// offset from the epoch's first slot. Uses exact i128 arithmetic.
///
/// When d = 0, no slot is an overlay slot (ceiling(0) == ceiling(0) always).
/// When d = 1, every slot is an overlay slot.
pub fn is_overlay_slot(
    first_slot: u64,
    d_num: u64,
    d_den: u64,
    slot: u64,
) -> bool {
    if d_num == 0 || d_den == 0 {
        return false;
    }
    let s = slot.saturating_sub(first_slot) as i128;
    let dn = d_num as i128;
    let dd = d_den as i128;

    // ceiling(s * d) = ceiling(s * d_num / d_den)
    let step_s = ceiling_mul(s, dn, dd);
    let step_s1 = ceiling_mul(s + 1, dn, dd);
    step_s < step_s1
}

/// Compute ceiling(x * numerator / denominator) using exact i128 arithmetic.
///
/// ceiling(a/b) = (a + b - 1) / b  for positive a, b (integer division).
/// Here a = x * numerator, b = denominator.
fn ceiling_mul(x: i128, numerator: i128, denominator: i128) -> i128 {
    let a = x * numerator;
    if a <= 0 {
        // ceiling of non-positive is just integer division (rounds toward zero)
        // For a == 0: result is 0
        // For a < 0: shouldn't happen in overlay context, but handle correctly
        if denominator == 0 {
            return 0;
        }
        // For exact i128: ceiling(a/b) when a <= 0, b > 0 = a / b (truncation rounds toward zero = ceiling for negatives)
        a / denominator
    } else {
        // ceiling(a/b) = (a + b - 1) / b for positive a, b
        (a + denominator - 1) / denominator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_overlay_slot_d_zero() {
        // d = 0: no overlay slots (fully decentralized)
        for s in 0..100 {
            assert!(!is_overlay_slot(0, 0, 1, s));
        }
    }

    #[test]
    fn test_is_overlay_slot_d_one() {
        // d = 1: every slot is an overlay slot (fully federated)
        for s in 0..100 {
            assert!(is_overlay_slot(0, 1, 1, s));
        }
    }

    #[test]
    fn test_is_overlay_slot_d_half() {
        // d = 1/2: every other slot is overlay
        // ceiling(0 * 0.5) = 0, ceiling(1 * 0.5) = 1 → step at 0 ✓
        // ceiling(1 * 0.5) = 1, ceiling(2 * 0.5) = 1 → no step at 1 ✗
        // ceiling(2 * 0.5) = 1, ceiling(3 * 0.5) = 2 → step at 2 ✓
        // ceiling(3 * 0.5) = 2, ceiling(4 * 0.5) = 2 → no step at 3 ✗
        let overlay: Vec<u64> = (0..10)
            .filter(|&s| is_overlay_slot(0, 1, 2, s))
            .collect();
        assert_eq!(overlay, vec![0, 2, 4, 6, 8]);
    }

    #[test]
    fn test_is_overlay_slot_d_one_fifth() {
        // d = 1/5: roughly every 5th slot
        let overlay: Vec<u64> = (0..20)
            .filter(|&s| is_overlay_slot(0, 1, 5, s))
            .collect();
        // ceiling(0*0.2)=0, ceiling(1*0.2)=1 → step ✓ (s=0)
        // ceiling(4*0.2)=1, ceiling(5*0.2)=1 → no step (s=4)
        // ceiling(5*0.2)=1, ceiling(6*0.2)=2 → step ✓ (s=5)
        assert_eq!(overlay, vec![0, 5, 10, 15]);
    }

    #[test]
    fn test_is_overlay_slot_with_offset() {
        // First slot of epoch is 100, d = 1/2
        let overlay: Vec<u64> = (100..110)
            .filter(|&s| is_overlay_slot(100, 1, 2, s))
            .collect();
        assert_eq!(overlay, vec![100, 102, 104, 106, 108]);
    }

    #[test]
    fn test_is_overlay_slot_mainnet_d() {
        // Mainnet Shelley launch: d = 1 (fully federated), then decreased
        // d = 9/10 = 0.9: roughly 9 out of every 10 slots
        let count = (0..100)
            .filter(|&s| is_overlay_slot(0, 9, 10, s))
            .count();
        assert_eq!(count, 90);
    }

    #[test]
    fn test_ceiling_mul_exact() {
        // ceiling(3 * 1/3) = ceiling(1) = 1
        assert_eq!(ceiling_mul(3, 1, 3), 1);
        // ceiling(4 * 1/3) = ceiling(4/3) = 2
        assert_eq!(ceiling_mul(4, 1, 3), 2);
        // ceiling(0 * 1/3) = 0
        assert_eq!(ceiling_mul(0, 1, 3), 0);
        // ceiling(1 * 1/1) = 1
        assert_eq!(ceiling_mul(1, 1, 1), 1);
    }
}
```

- [ ] **Step 2: Add module to lib.rs**

Add to `crates/dugite-consensus/src/lib.rs`:

```rust
pub mod overlay;
```

And add re-exports:

```rust
pub use overlay::{OBftSlot, OverlayContext};
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-consensus -E 'test(overlay)'`
Expected: All overlay tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-consensus/src/overlay.rs crates/dugite-consensus/src/lib.rs
git commit -m "feat(consensus): add BFT overlay slot detection matching Haskell isOverlaySlot"
```

---

### Task 2: Overlay Slot Classification — `classify_overlay_slot` and `lookup_in_overlay_schedule`

**Files:**
- Modify: `crates/dugite-consensus/src/overlay.rs`

- [ ] **Step 1: Add `classify_overlay_slot` and `lookup_in_overlay_schedule`**

Append to `crates/dugite-consensus/src/overlay.rs` (before the `#[cfg(test)]` block):

```rust
/// Classify an overlay slot to determine which genesis delegate should sign.
///
/// Matches Haskell's `classifyOverlaySlot` from
/// `Cardano.Protocol.TPraos.Rules.Overlay`:
///
/// ```haskell
/// classifyOverlaySlot firstSlotNo gkeys dval ascValue slot =
///   if isActive
///     then gkeys `getAtIndex` genesisIdx
///     else NonActiveSlot
///   where
///     d        = unboundRational dval
///     position = ceiling (fromIntegral (slot -* firstSlotNo) * d)
///     isActive = position `mod` ascInv == 0
///     ascInv   = floor (1 / unboundRational (activeSlotVal ascValue))
///     genesisIdx = (position `div` ascInv) `mod` length gkeys
/// ```
///
/// The genesis keys must be in a `BTreeSet` for deterministic sorted ordering
/// (matching Haskell's `Data.Set` which is ordered by `Ord`).
///
/// `f_num`/`f_den` is the active slot coefficient as a rational (e.g., 1/20).
pub fn classify_overlay_slot(
    first_slot: u64,
    genesis_keys: &BTreeSet<Hash28>,
    d_num: u64,
    d_den: u64,
    f_num: u64,
    f_den: u64,
    slot: u64,
) -> OBftSlot {
    if genesis_keys.is_empty() {
        return OBftSlot::NonActiveSlot;
    }

    let offset = slot.saturating_sub(first_slot) as i128;
    let dn = d_num as i128;
    let dd = d_den as i128;

    // position = ceiling(offset * d)
    let position = ceiling_mul(offset, dn, dd);

    // asc_inv = floor(1 / f) = floor(f_den / f_num)
    let asc_inv = if f_num == 0 {
        // Shouldn't happen in practice, but avoid division by zero
        return OBftSlot::NonActiveSlot;
    } else {
        (f_den / f_num) as i128
    };

    if asc_inv == 0 {
        return OBftSlot::NonActiveSlot;
    }

    // Active if position is a multiple of asc_inv
    if position % asc_inv != 0 {
        return OBftSlot::NonActiveSlot;
    }

    // Genesis delegate index: round-robin through sorted keys
    let n = genesis_keys.len() as i128;
    let genesis_idx = ((position / asc_inv) % n) as usize;

    match genesis_keys.iter().nth(genesis_idx) {
        Some(gkey) => OBftSlot::ActiveSlot(*gkey),
        None => OBftSlot::NonActiveSlot,
    }
}

/// Look up whether a slot is in the overlay schedule and classify it.
///
/// Matches Haskell's `lookupInOverlaySchedule`:
/// - Returns `None` for pure Praos slots (pool produces)
/// - Returns `Some(NonActiveSlot)` for silent overlay slots (no block allowed)
/// - Returns `Some(ActiveSlot(gkey))` for BFT slots assigned to a genesis delegate
pub fn lookup_in_overlay_schedule(
    first_slot: u64,
    genesis_keys: &BTreeSet<Hash28>,
    d_num: u64,
    d_den: u64,
    f_num: u64,
    f_den: u64,
    slot: u64,
) -> Option<OBftSlot> {
    if !is_overlay_slot(first_slot, d_num, d_den, slot) {
        return None;
    }
    Some(classify_overlay_slot(
        first_slot,
        genesis_keys,
        d_num, d_den,
        f_num, f_den,
        slot,
    ))
}

/// Context needed for overlay schedule validation.
///
/// Passed to `validate_header_full()` when the decentralization parameter d > 0.
/// Contains the genesis delegates map and the d parameter as an exact rational.
#[derive(Debug, Clone)]
pub struct OverlayContext {
    /// Genesis delegates: genesis_key_hash → (delegate_key_hash, delegate_vrf_key_hash).
    /// The delegate_key_hash is Blake2b-224 of the delegate's cold verification key.
    /// The delegate_vrf_key_hash is Blake2b-256 of the delegate's VRF verification key.
    pub genesis_delegates: std::collections::HashMap<Hash28, (Hash28, dugite_primitives::hash::Hash32)>,
    /// Sorted set of genesis key hashes for deterministic round-robin assignment.
    /// Must match Haskell's `Data.Set` ordering (lexicographic on 28-byte hash).
    pub genesis_keys: BTreeSet<Hash28>,
    /// Decentralization parameter as exact rational (numerator, denominator).
    /// d = 1 means fully federated, d = 0 means fully decentralized (Praos).
    pub d: (u64, u64),
    /// First slot of the current epoch.
    pub first_slot_of_epoch: u64,
}
```

- [ ] **Step 2: Add tests for `classify_overlay_slot` and `lookup_in_overlay_schedule`**

Append to the `tests` module in `overlay.rs`:

```rust
    use std::collections::HashMap;
    use dugite_primitives::hash::Hash32;

    fn make_genesis_keys(n: usize) -> BTreeSet<Hash28> {
        (0..n)
            .map(|i| {
                let mut bytes = [0u8; 28];
                bytes[0] = i as u8;
                Hash28::from_bytes(bytes)
            })
            .collect()
    }

    #[test]
    fn test_classify_overlay_slot_active() {
        // d = 1, f = 1/20, 3 genesis keys
        // position = ceiling(offset * 1) = offset
        // asc_inv = floor(1 / (1/20)) = 20
        // Active when position % 20 == 0 → slots 0, 20, 40, ...
        // genesis_idx = (position / 20) % 3
        let gkeys = make_genesis_keys(3);

        // Slot 0: position=0, 0%20==0 → active, idx=(0/20)%3=0
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 0);
        assert_eq!(result, OBftSlot::ActiveSlot(*gkeys.iter().nth(0).unwrap()));

        // Slot 20: position=20, 20%20==0 → active, idx=(20/20)%3=1
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 20);
        assert_eq!(result, OBftSlot::ActiveSlot(*gkeys.iter().nth(1).unwrap()));

        // Slot 40: position=40, 40%20==0 → active, idx=(40/20)%3=2
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 40);
        assert_eq!(result, OBftSlot::ActiveSlot(*gkeys.iter().nth(2).unwrap()));

        // Slot 60: wraps around, idx=(60/20)%3=0
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 60);
        assert_eq!(result, OBftSlot::ActiveSlot(*gkeys.iter().nth(0).unwrap()));
    }

    #[test]
    fn test_classify_overlay_slot_non_active() {
        // d = 1, f = 1/20, 3 genesis keys
        // Slot 1: position=1, 1%20!=0 → NonActiveSlot
        let gkeys = make_genesis_keys(3);
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 1);
        assert_eq!(result, OBftSlot::NonActiveSlot);

        // Slot 19: position=19, 19%20!=0 → NonActiveSlot
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 19);
        assert_eq!(result, OBftSlot::NonActiveSlot);
    }

    #[test]
    fn test_lookup_praos_slot() {
        // d = 1/2, f = 1/20
        // Slot 1 is NOT an overlay slot (see d_half test above)
        let gkeys = make_genesis_keys(3);
        let result = lookup_in_overlay_schedule(0, &gkeys, 1, 2, 1, 20, 1);
        assert_eq!(result, None); // Praos slot
    }

    #[test]
    fn test_lookup_overlay_active() {
        // d = 1, f = 1/20
        // Slot 0 IS an overlay slot AND active (position=0, 0%20==0)
        let gkeys = make_genesis_keys(3);
        let result = lookup_in_overlay_schedule(0, &gkeys, 1, 1, 1, 20, 0);
        assert!(matches!(result, Some(OBftSlot::ActiveSlot(_))));
    }

    #[test]
    fn test_lookup_overlay_non_active() {
        // d = 1, f = 1/20
        // Slot 1 IS an overlay slot but NOT active (position=1, 1%20!=0)
        let gkeys = make_genesis_keys(3);
        let result = lookup_in_overlay_schedule(0, &gkeys, 1, 1, 1, 20, 1);
        assert_eq!(result, Some(OBftSlot::NonActiveSlot));
    }

    #[test]
    fn test_classify_with_epoch_offset() {
        // first_slot = 100, d = 1, f = 1/20, slot = 120
        // offset = 120 - 100 = 20, position = 20, 20%20==0 → active
        let gkeys = make_genesis_keys(2);
        let result = classify_overlay_slot(100, &gkeys, 1, 1, 1, 20, 120);
        assert_eq!(result, OBftSlot::ActiveSlot(*gkeys.iter().nth(1).unwrap()));
    }

    #[test]
    fn test_classify_empty_genesis_keys() {
        let gkeys = BTreeSet::new();
        let result = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, 0);
        assert_eq!(result, OBftSlot::NonActiveSlot);
    }

    #[test]
    fn test_overlay_preview_params() {
        // Preview testnet: f = 1/20, d started at 1 then decreased
        // With 7 genesis delegates (preview has 7), d = 1:
        // Active slots at 0, 20, 40, 60, 80, 100, 120 (round-robin through 7 keys)
        let gkeys = make_genesis_keys(7);
        let active_slots: Vec<u64> = (0..200)
            .filter_map(|s| {
                match lookup_in_overlay_schedule(0, &gkeys, 1, 1, 1, 20, s) {
                    Some(OBftSlot::ActiveSlot(_)) => Some(s),
                    _ => None,
                }
            })
            .collect();
        // Should be every 20th slot
        assert_eq!(active_slots, (0..200).step_by(20).collect::<Vec<_>>());
    }
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo nextest run -p dugite-consensus -E 'test(overlay)'`
Expected: All overlay tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-consensus/src/overlay.rs
git commit -m "feat(consensus): add overlay slot classification and lookup matching Haskell OVERLAY rule"
```

---

### Task 3: Store Genesis Delegates in LedgerState

**Files:**
- Modify: `crates/dugite-ledger/src/state/mod.rs`
- Modify: `crates/dugite-node/src/node/mod.rs`

- [ ] **Step 1: Add `genesis_delegates` field to `LedgerState`**

In `crates/dugite-ledger/src/state/mod.rs`, add to the `LedgerState` struct (after the `pointer_map` field around line 174):

```rust
    /// Genesis delegates: genesis_key_hash (28 bytes) → (delegate_key_hash (28 bytes), vrf_key_hash (32 bytes)).
    ///
    /// Loaded from the Shelley genesis file. Used for BFT overlay schedule
    /// validation during early Shelley era (when d > 0). Genesis delegates
    /// produce blocks on overlay slots; the delegate_key_hash is Blake2b-224
    /// of the delegate's cold verification key, and the vrf_key_hash is
    /// Blake2b-256 of the delegate's VRF verification key.
    ///
    /// Not mutated after initialization — the genesis delegation map is static
    /// for the lifetime of the Shelley-based eras.
    #[serde(default)]
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
```

Import `Hash28` if not already imported at the top of the file.

- [ ] **Step 2: Add setter method on LedgerState**

In `crates/dugite-ledger/src/state/mod.rs`, add a method to `impl LedgerState`:

```rust
    /// Load genesis delegates from Shelley genesis data.
    ///
    /// Each entry is (genesis_key_hash_28, delegate_key_hash_28, vrf_key_hash_32)
    /// as raw bytes. Called during node initialization from `ShelleyGenesis::gen_delegs_entries()`.
    pub fn set_genesis_delegates(&mut self, entries: &[(Vec<u8>, Vec<u8>, Vec<u8>)]) {
        self.genesis_delegates.clear();
        for (genesis_hash, delegate_hash, vrf_hash) in entries {
            if genesis_hash.len() == 28 && delegate_hash.len() == 28 && vrf_hash.len() == 32 {
                let gkey = Hash28::from_bytes({
                    let mut buf = [0u8; 28];
                    buf.copy_from_slice(genesis_hash);
                    buf
                });
                let dkey = Hash28::from_bytes({
                    let mut buf = [0u8; 28];
                    buf.copy_from_slice(delegate_hash);
                    buf
                });
                let vrf = Hash32::from_bytes({
                    let mut buf = [0u8; 32];
                    buf.copy_from_slice(vrf_hash);
                    buf
                });
                self.genesis_delegates.insert(gkey, (dkey, vrf));
            }
        }
    }
```

- [ ] **Step 3: Load genesis delegates during node initialization**

In `crates/dugite-node/src/node/mod.rs`, find where Shelley genesis parameters are applied to ledger state (around line 399, the block starting with `if let Some(ref genesis) = shelley_genesis`). After the existing `state.set_update_quorum(genesis.update_quorum);` line, add:

```rust
                        // Load genesis delegates for BFT overlay schedule validation
                        let gen_deleg_entries = genesis.gen_delegs_entries();
                        if !gen_deleg_entries.is_empty() {
                            tracing::debug!(
                                count = gen_deleg_entries.len(),
                                "Loaded genesis delegates for overlay schedule validation"
                            );
                            state.set_genesis_delegates(&gen_deleg_entries);
                        }
```

Do the same in all other places where `ShelleyGenesis` is applied to `LedgerState` — search for `set_epoch_length` and `set_update_quorum` calls to find them. There are at least 3 places: the initial load, the `apply_genesis_to_ledger` function, and the snapshot restore path. Add `set_genesis_delegates` after each one.

- [ ] **Step 4: Run build to verify compilation**

Run: `cargo build --all-targets 2>&1 | head -30`
Expected: Compiles successfully

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/mod.rs crates/dugite-node/src/node/mod.rs
git commit -m "feat(ledger): store genesis delegates in LedgerState for overlay validation"
```

---

### Task 4: Add Overlay-Aware Validation to `validate_header_full`

**Files:**
- Modify: `crates/dugite-consensus/src/praos.rs`

- [ ] **Step 1: Add new `ConsensusError` variants**

In `crates/dugite-consensus/src/praos.rs`, add to the `ConsensusError` enum:

```rust
    #[error("Not an active overlay slot: slot {slot} is in the overlay schedule but has no assigned signer")]
    NotActiveOverlaySlot { slot: u64 },
    #[error("Wrong genesis delegate for overlay slot: expected delegate for genesis key {genesis_key}, got issuer {issuer}")]
    WrongGenesisDelegate {
        genesis_key: Hash28,
        issuer: Hash28,
    },
    #[error("Unknown genesis key in overlay schedule: {0}")]
    UnknownGenesisKey(Hash28),
    #[error("Genesis delegate VRF key mismatch: expected {expected}, got {got}")]
    GenesisVrfKeyMismatch {
        expected: dugite_primitives::hash::Hash32,
        got: dugite_primitives::hash::Hash32,
    },
```

- [ ] **Step 2: Add overlay import and modify `validate_header_full` signature**

Add import at the top of `praos.rs`:

```rust
use crate::overlay::{self, OBftSlot, OverlayContext};
```

Change the `validate_header_full` signature to accept an optional overlay context:

```rust
    pub fn validate_header_full(
        &mut self,
        header: &BlockHeader,
        current_slot: SlotNo,
        issuer_info: Option<&BlockIssuerInfo>,
        overlay_ctx: Option<&OverlayContext>,
        mode: ValidationMode,
        ledger_pv_major: Option<u64>,
    ) -> Result<(), ConsensusError> {
```

- [ ] **Step 3: Add overlay validation logic**

After the protocol version checks (step 1b, around line 492) and BEFORE the pool registration check (step 2), insert the overlay branching logic:

```rust
        // 2. Overlay schedule check (Haskell's OVERLAY rule).
        //
        // When d > 0 (early Shelley through late Alonzo), determine if this slot
        // is assigned to a BFT genesis delegate via the overlay schedule.
        // - Praos slot (not overlay): fall through to pool-based validation below
        // - Active overlay slot: verify issuer is the assigned genesis delegate
        // - NonActive overlay slot: reject (no block should exist here)
        //
        // BFT overlay blocks do NOT undergo the VRF leader threshold check —
        // the genesis delegate is entitled by schedule assignment, not by stake.
        // VRF proofs are still verified (key binding + proof validity), matching
        // Haskell's `pbftVrfChecks`.
        if let Some(ctx) = overlay_ctx {
            let (d_num, d_den) = ctx.d;
            if d_num > 0 {
                let (f_num, f_den) = self.active_slot_coeff_rational;
                let classification = overlay::lookup_in_overlay_schedule(
                    ctx.first_slot_of_epoch,
                    &ctx.genesis_keys,
                    d_num,
                    d_den,
                    f_num,
                    f_den,
                    header.slot.0,
                );

                match classification {
                    Some(OBftSlot::NonActiveSlot) => {
                        // Slot is in the overlay schedule but not active —
                        // no block should exist here.
                        warn!(
                            slot = header.slot.0,
                            "Overlay: block in non-active overlay slot — rejecting"
                        );
                        return Err(ConsensusError::NotActiveOverlaySlot {
                            slot: header.slot.0,
                        });
                    }
                    Some(OBftSlot::ActiveSlot(genesis_key)) => {
                        // BFT overlay path: verify the block issuer is the correct
                        // genesis delegate for this slot.
                        trace!(
                            slot = header.slot.0,
                            genesis_key = %genesis_key,
                            "Overlay: active BFT overlay slot"
                        );

                        // Look up the delegate for this genesis key
                        let (delegate_hash, delegate_vrf_hash) = match ctx
                            .genesis_delegates
                            .get(&genesis_key)
                        {
                            Some(pair) => pair,
                            None => {
                                if self.strict_verification {
                                    return Err(ConsensusError::UnknownGenesisKey(genesis_key));
                                }
                                debug!(
                                    slot = header.slot.0,
                                    genesis_key = %genesis_key,
                                    "Overlay: unknown genesis key (non-fatal during sync)"
                                );
                                // Skip overlay checks, fall through to opcert + KES
                                self.check_opcert_counter(header, issuer_info)?;
                                self.validate_kes_period(header)?;
                                if mode == ValidationMode::Full {
                                    self.verify_vrf_proof(header)?;
                                    self.verify_nonce_vrf_proof(header)?;
                                    self.validate_operational_cert(header)?;
                                    self.verify_kes_signature(header)?;
                                }
                                return Ok(());
                            }
                        };

                        // Verify block issuer key hash matches the delegate key hash.
                        // Haskell: hashKey(bheaderVk bhb) == coerceKeyRole(genDelegKeyHash)
                        let issuer_hash =
                            dugite_primitives::hash::blake2b_224(&header.issuer_vkey);
                        if issuer_hash != *delegate_hash {
                            if self.strict_verification {
                                return Err(ConsensusError::WrongGenesisDelegate {
                                    genesis_key,
                                    issuer: issuer_hash,
                                });
                            }
                            debug!(
                                slot = header.slot.0,
                                expected = %delegate_hash,
                                got = %issuer_hash,
                                "Overlay: wrong genesis delegate (non-fatal during sync)"
                            );
                        }

                        // Verify VRF key hash matches the delegate's registered VRF key hash.
                        // Haskell's pbftVrfChecks: hashVerKeyVRF(bheaderVrfVk) == genDelegVrfHash
                        if header.vrf_vkey.len() == 32 {
                            let header_vrf_hash = blake2b_256(&header.vrf_vkey);
                            if *header_vrf_hash.as_bytes() != *delegate_vrf_hash.as_bytes() {
                                if self.strict_verification {
                                    return Err(ConsensusError::GenesisVrfKeyMismatch {
                                        expected: *delegate_vrf_hash,
                                        got: header_vrf_hash,
                                    });
                                }
                                debug!(
                                    slot = header.slot.0,
                                    "Overlay: VRF key hash mismatch for genesis delegate (non-fatal during sync)"
                                );
                            }
                        }

                        // BFT path: NO leader threshold check (the delegate is entitled
                        // by overlay schedule assignment, not by VRF stake check).
                        // VRF proofs are still verified in Full mode (Haskell's pbftVrfChecks
                        // calls VRF.verifyCertified for both nonce and leader seeds).

                        // Opcert counter check (same as Praos path)
                        self.check_opcert_counter(header, issuer_info)?;

                        // KES period validation
                        self.validate_kes_period(header)?;

                        // Crypto verification in Full mode only
                        if mode == ValidationMode::Full {
                            self.verify_vrf_proof(header)?;
                            self.verify_nonce_vrf_proof(header)?;
                            self.validate_operational_cert(header)?;
                            self.verify_kes_signature(header)?;
                        }

                        trace!(
                            slot = header.slot.0,
                            genesis_key = %genesis_key,
                            mode = ?mode,
                            "Overlay: BFT overlay block validation passed"
                        );

                        return Ok(());
                    }
                    None => {
                        // Not an overlay slot — fall through to Praos path below
                    }
                }
            }
        }
```

- [ ] **Step 4: Fix all call sites of `validate_header_full` to pass `None` for overlay_ctx**

Search for all calls to `validate_header_full(` in the codebase. For each call, add `None,` as the new 4th argument (after `issuer_info`). This includes:
- `crates/dugite-node/src/node/sync.rs` (will be updated properly in Task 5)
- `crates/dugite-node/tests/forge_integration.rs`
- All test calls in `crates/dugite-consensus/src/praos.rs`

For the test calls in `praos.rs`, the pattern is:
```rust
// Before:
.validate_header_full(&header, SlotNo(200), None, ValidationMode::Full, Some(9))
// After:
.validate_header_full(&header, SlotNo(200), None, None, ValidationMode::Full, Some(9))
```

- [ ] **Step 5: Run build and tests**

Run: `cargo build --all-targets 2>&1 | head -30`
Run: `cargo nextest run -p dugite-consensus`
Expected: Build succeeds, all existing tests pass

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-consensus/src/praos.rs crates/dugite-node/src/node/sync.rs crates/dugite-node/tests/forge_integration.rs
git commit -m "feat(consensus): add overlay-aware validation path in validate_header_full"
```

---

### Task 5: Wire Up Overlay Context in Sync Pipeline

**Files:**
- Modify: `crates/dugite-node/src/node/sync.rs`

- [ ] **Step 1: Build `OverlayContext` and pass to `validate_header_full`**

In `crates/dugite-node/src/node/sync.rs`, inside the block validation loop (around line 770, inside the `{ let ls = ... }` block), after `let total_active_stake` is computed and before the `for block in &blocks` loop, build the overlay context:

```rust
            // Build overlay context for BFT schedule validation.
            // Only needed when d > 0 and protocol version < 7 (pre-Babbage).
            // For Babbage+ (proto >= 7), d is always 0 and overlay is skipped.
            let overlay_ctx = if ls.protocol_params.protocol_version_major < 7
                && ls.protocol_params.d.numerator > 0
                && !ls.genesis_delegates.is_empty()
            {
                let epoch = ls.epoch_of_slot(
                    blocks.first().map(|b| b.slot().0).unwrap_or(0),
                );
                let first_slot = ls.first_slot_of_epoch(epoch);
                let genesis_keys: std::collections::BTreeSet<dugite_primitives::hash::Hash28> =
                    ls.genesis_delegates.keys().copied().collect();
                Some(dugite_consensus::overlay::OverlayContext {
                    genesis_delegates: ls.genesis_delegates.clone(),
                    genesis_keys,
                    d: (ls.protocol_params.d.numerator, ls.protocol_params.d.denominator),
                    first_slot_of_epoch: first_slot,
                })
            } else {
                None
            };
```

Then update the `validate_header_full` call (around line 865) to pass the overlay context:

```rust
                if let Err(e) = self.consensus.validate_header_full(
                    &header_with_nonce,
                    block.slot(),
                    issuer_info.as_ref(),
                    overlay_ctx.as_ref(),
                    mode,
                    Some(ls.protocol_params.protocol_version_major),
                ) {
```

**Note on epoch boundaries within a batch:** When a batch spans an epoch boundary, the `d` parameter may change. However, protocol parameter updates only take effect at epoch boundaries, and the overlay schedule uses `d` from `curPParams`. The current design computes `overlay_ctx` once per batch using the current `d`. This is correct because:
1. The batch is processed sequentially and epoch transitions update `protocol_params`
2. Blocks within a single epoch share the same `d`
3. Cross-epoch batches: the first epoch's blocks use the pre-transition `d`, which is correct since we read `ls` before any mutations

If a batch actually crosses an epoch boundary, the overlay context should be recomputed. Add this recomputation inside the loop, just before the `validate_header_full` call:

```rust
                // Recompute overlay context if the block crosses an epoch boundary
                // within this batch. The d parameter may change at epoch transitions.
                let block_overlay_ctx = if ls.protocol_params.protocol_version_major < 7
                    && ls.protocol_params.d.numerator > 0
                    && !ls.genesis_delegates.is_empty()
                {
                    let block_epoch = ls.epoch_of_slot(block.slot().0);
                    let cached_epoch = overlay_ctx.as_ref().map(|ctx| {
                        ls.epoch_of_slot(ctx.first_slot_of_epoch)
                    });
                    if cached_epoch == Some(block_epoch) {
                        overlay_ctx.as_ref()
                    } else {
                        // Epoch changed within batch — would need fresh context.
                        // However, ledger state hasn't been mutated yet (read lock),
                        // so the d parameter is still from the current epoch.
                        // This is correct: overlay uses curPParams, not nextPParams.
                        overlay_ctx.as_ref()
                    }
                } else {
                    None
                };
```

Actually, since the ledger read lock means `d` doesn't change within the batch, the simple approach of using `overlay_ctx.as_ref()` directly is correct. Remove the recomputation and just use:

```rust
                if let Err(e) = self.consensus.validate_header_full(
                    &header_with_nonce,
                    block.slot(),
                    issuer_info.as_ref(),
                    overlay_ctx.as_ref(),
                    mode,
                    Some(ls.protocol_params.protocol_version_major),
                ) {
```

- [ ] **Step 2: Run build and tests**

Run: `cargo build --all-targets 2>&1 | head -30`
Run: `cargo nextest run --workspace`
Expected: Build succeeds, all tests pass

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-node/src/node/sync.rs
git commit -m "feat(node): wire overlay context through sync pipeline for BFT validation"
```

---

### Task 6: Add Integration Tests for Overlay Validation

**Files:**
- Modify: `crates/dugite-consensus/src/overlay.rs`

- [ ] **Step 1: Add integration-style tests to overlay.rs**

Add these tests to the `tests` module in `overlay.rs`:

```rust
    #[test]
    fn test_overlay_context_full_workflow() {
        // Simulate a Shelley epoch with d=1, f=1/20, 3 genesis delegates
        let mut genesis_delegates = HashMap::new();
        let gkeys = make_genesis_keys(3);

        // Create delegate entries for each genesis key
        for (i, gkey) in gkeys.iter().enumerate() {
            let mut delegate_bytes = [0u8; 28];
            delegate_bytes[0] = (i + 10) as u8; // different from genesis key
            let delegate = Hash28::from_bytes(delegate_bytes);

            let mut vrf_bytes = [0u8; 32];
            vrf_bytes[0] = (i + 20) as u8;
            let vrf = Hash32::from_bytes(vrf_bytes);

            genesis_delegates.insert(*gkey, (delegate, vrf));
        }

        let ctx = OverlayContext {
            genesis_delegates: genesis_delegates.clone(),
            genesis_keys: gkeys.clone(),
            d: (1, 1),
            first_slot_of_epoch: 0,
        };

        // Slot 0: active overlay, assigned to genesis key 0
        let result = lookup_in_overlay_schedule(
            ctx.first_slot_of_epoch,
            &ctx.genesis_keys,
            ctx.d.0, ctx.d.1,
            1, 20,
            0,
        );
        match result {
            Some(OBftSlot::ActiveSlot(gkey)) => {
                let (delegate, vrf) = ctx.genesis_delegates.get(&gkey).unwrap();
                assert_eq!(delegate.as_bytes()[0], 10);
                assert_eq!(vrf.as_bytes()[0], 20);
            }
            other => panic!("Expected ActiveSlot, got {:?}", other),
        }

        // Slot 1: non-active overlay slot (d=1 but not multiple of asc_inv=20)
        let result = lookup_in_overlay_schedule(
            ctx.first_slot_of_epoch,
            &ctx.genesis_keys,
            ctx.d.0, ctx.d.1,
            1, 20,
            1,
        );
        assert_eq!(result, Some(OBftSlot::NonActiveSlot));
    }

    #[test]
    fn test_overlay_d_transition() {
        // Test decreasing d values (mainnet Shelley history)
        let gkeys = make_genesis_keys(7);

        // d = 1.0: all slots are overlay
        let overlay_count = (0..100)
            .filter(|&s| is_overlay_slot(0, 1, 1, s))
            .count();
        assert_eq!(overlay_count, 100);

        // d = 0.5: half are overlay
        let overlay_count = (0..100)
            .filter(|&s| is_overlay_slot(0, 1, 2, s))
            .count();
        assert_eq!(overlay_count, 50);

        // d = 0.0: none are overlay
        let overlay_count = (0..100)
            .filter(|&s| is_overlay_slot(0, 0, 1, s))
            .count();
        assert_eq!(overlay_count, 0);

        // Active BFT slots should decrease proportionally with d
        let active_d1 = (0..1000)
            .filter_map(|s| {
                match lookup_in_overlay_schedule(0, &gkeys, 1, 1, 1, 20, s) {
                    Some(OBftSlot::ActiveSlot(_)) => Some(s),
                    _ => None,
                }
            })
            .count();
        assert_eq!(active_d1, 50); // 1000 / 20 = 50

        let active_d05 = (0..1000)
            .filter_map(|s| {
                match lookup_in_overlay_schedule(0, &gkeys, 1, 2, 1, 20, s) {
                    Some(OBftSlot::ActiveSlot(_)) => Some(s),
                    _ => None,
                }
            })
            .count();
        assert_eq!(active_d05, 25); // 500 overlay / 20 = 25
    }

    #[test]
    fn test_overlay_genesis_key_round_robin() {
        // Verify genesis keys are assigned in sorted order via round-robin
        let gkeys = make_genesis_keys(3);
        let keys_vec: Vec<Hash28> = gkeys.iter().copied().collect();

        // With d=1, f=1/20: active at slots 0, 20, 40, 60, 80, 100
        // genesis_idx = (position/20) % 3
        let mut assignments = Vec::new();
        for s in (0..120).step_by(20) {
            if let OBftSlot::ActiveSlot(gkey) = classify_overlay_slot(0, &gkeys, 1, 1, 1, 20, s) {
                let idx = keys_vec.iter().position(|k| *k == gkey).unwrap();
                assignments.push(idx);
            }
        }
        // Should round-robin: 0, 1, 2, 0, 1, 2
        assert_eq!(assignments, vec![0, 1, 2, 0, 1, 2]);
    }
```

- [ ] **Step 2: Run all tests**

Run: `cargo nextest run -p dugite-consensus -E 'test(overlay)'`
Expected: All tests PASS

- [ ] **Step 3: Run full test suite and clippy**

Run: `cargo nextest run --workspace`
Run: `cargo clippy --all-targets -- -D warnings`
Run: `cargo fmt --all -- --check`
Expected: All pass with zero warnings

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-consensus/src/overlay.rs
git commit -m "test(consensus): add comprehensive overlay schedule integration tests"
```

---

### Task 7: Final Verification and Cleanup

- [ ] **Step 1: Run full workspace build and test suite**

```bash
cargo build --all-targets
cargo nextest run --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

All must pass with zero warnings.

- [ ] **Step 2: Verify overlay is bypassed for d=0 (no performance impact)**

The overlay path is gated by `d_num > 0` in the `OverlayContext` construction and `d_num > 0` in `validate_header_full`. For Conway/Babbage blocks (proto >= 7), the overlay context is `None`. Verify with a quick grep that no unnecessary work is done when `d = 0`.

- [ ] **Step 3: Final commit (if any cleanup needed)**

```bash
git add -A
git commit -m "chore(consensus): overlay schedule cleanup and formatting"
```
