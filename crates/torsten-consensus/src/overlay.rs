//! BFT overlay schedule computation for Shelley-era blocks.
//!
//! When the decentralization parameter `d > 0`, a fraction of slots are
//! designated as "overlay" slots where genesis delegates (BFT nodes) produce
//! blocks. This module implements the pure schedule computation matching
//! Haskell's `OVERLAY` rule from `cardano-ledger`.
//!
//! The overlay schedule works as follows:
//! - `d` (decentralization parameter) controls what fraction of slots are overlay.
//!   When `d=1`, all slots are overlay (fully federated). When `d=0`, no slots
//!   are overlay (fully decentralized Praos).
//! - `f` (active slot coefficient) determines how often an overlay slot is
//!   "active" (assigned to a specific genesis delegate) vs "non-active" (silent).
//! - Genesis delegates are assigned to active slots in sorted round-robin order.

use std::collections::{BTreeSet, HashMap};
use torsten_primitives::hash::{Hash28, Hash32};

/// Result of classifying an overlay slot, matching Haskell's `OBftSlot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OBftSlot {
    /// An overlay slot with no assigned signer (silent/non-active slot).
    NonActiveSlot,
    /// An overlay slot assigned to a specific genesis key hash.
    ActiveSlot(Hash28),
}

/// Context needed for overlay schedule lookups within an epoch.
#[derive(Debug, Clone)]
pub struct OverlayContext {
    /// Mapping from genesis key hash to (delegate key hash, delegate VRF key hash).
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    /// Sorted set of genesis key hashes (BTreeSet gives deterministic order matching Haskell).
    pub genesis_keys: BTreeSet<Hash28>,
    /// Decentralization parameter as a rational `(numerator, denominator)`.
    /// `d = 1` means fully federated, `d = 0` means fully decentralized.
    pub d: (u64, u64),
    /// The absolute slot number of the first slot in the current epoch.
    pub first_slot_of_epoch: u64,
}

/// Compute `ceil(x * numerator / denominator)` using exact i128 arithmetic.
///
/// This avoids floating-point rounding errors that could cause disagreement
/// with the Haskell reference implementation.
///
/// # Panics
///
/// Panics if `denominator` is zero.
fn ceiling_mul(x: i128, numerator: i128, denominator: i128) -> i128 {
    assert!(denominator != 0, "ceiling_mul: denominator must not be zero");
    let product = x * numerator;
    // ceil(a / b) for positive a, b = (a + b - 1) / b
    // For general case, we use: ceil(a/b) = floor(a/b) + if a % b != 0 { 1 } else { 0 }
    // But we need to handle sign correctly. In Cardano, all values are non-negative.
    let (quot, rem) = (product / denominator, product % denominator);
    if rem > 0 {
        quot + 1
    } else {
        quot
    }
}

/// Determine whether a given slot is an overlay slot.
///
/// Matches Haskell's `isOverlaySlot`: a slot is an overlay slot if and only if
/// `ceil(s * d) < ceil((s + 1) * d)` where `s = slot - first_slot` is the
/// offset within the epoch and `d = d_num / d_den`.
///
/// When `d = 0`, no slots are overlay. When `d = 1`, all slots are overlay.
pub fn is_overlay_slot(first_slot: u64, d_num: u64, d_den: u64, slot: u64) -> bool {
    // d = 0 means fully decentralized, no overlay slots
    if d_num == 0 || d_den == 0 {
        return false;
    }

    let s = slot.saturating_sub(first_slot) as i128;
    let num = d_num as i128;
    let den = d_den as i128;

    // A slot is overlay iff ceil(s * d) < ceil((s + 1) * d)
    let left = ceiling_mul(s, num, den);
    let right = ceiling_mul(s + 1, num, den);
    left < right
}

/// Classify an overlay slot as active (assigned to a genesis delegate) or non-active.
///
/// Matches Haskell's `classifyOverlaySlot`:
/// - `position = ceil(offset * d)` where offset = slot - first_slot
/// - `asc_inv = floor(1 / f) = f_den / f_num` (inverse of active slot coefficient)
/// - A slot is active if `position % asc_inv == 0`
/// - The genesis key index is `(position / asc_inv) % n_keys` into the sorted key set
///
/// # Parameters
///
/// - `first_slot` - First slot of the epoch
/// - `genesis_keys` - Sorted set of genesis key hashes
/// - `d_num`, `d_den` - Decentralization parameter as rational
/// - `f_num`, `f_den` - Active slot coefficient as rational
/// - `slot` - The slot to classify
pub fn classify_overlay_slot(
    first_slot: u64,
    genesis_keys: &BTreeSet<Hash28>,
    d_num: u64,
    d_den: u64,
    f_num: u64,
    f_den: u64,
    slot: u64,
) -> OBftSlot {
    let n_keys = genesis_keys.len();
    if n_keys == 0 {
        return OBftSlot::NonActiveSlot;
    }

    let offset = slot.saturating_sub(first_slot) as i128;
    let position = ceiling_mul(offset, d_num as i128, d_den as i128);

    // asc_inv = floor(1/f) = f_den / f_num (integer division = floor for positive values)
    let asc_inv = if f_num == 0 {
        return OBftSlot::NonActiveSlot;
    } else {
        (f_den / f_num) as i128
    };

    if asc_inv == 0 || position % asc_inv != 0 {
        return OBftSlot::NonActiveSlot;
    }

    // Round-robin assignment: index into sorted genesis keys
    let key_index = ((position / asc_inv) % (n_keys as i128)) as usize;
    let key = genesis_keys.iter().nth(key_index).unwrap();
    OBftSlot::ActiveSlot(*key)
}

/// Look up whether a slot is in the overlay schedule and, if so, classify it.
///
/// Combines `is_overlay_slot` and `classify_overlay_slot`:
/// - Returns `None` for Praos slots (not in the overlay schedule)
/// - Returns `Some(NonActiveSlot)` for silent overlay slots
/// - Returns `Some(ActiveSlot(gkey))` for BFT-assigned overlay slots
///
/// # Parameters
///
/// - `first_slot` - First slot of the epoch
/// - `genesis_keys` - Sorted set of genesis key hashes
/// - `d_num`, `d_den` - Decentralization parameter as rational
/// - `f_num`, `f_den` - Active slot coefficient as rational
/// - `slot` - The slot to look up
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
        d_num,
        d_den,
        f_num,
        f_den,
        slot,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a Hash28 from a single byte value (for test convenience).
    fn make_key(b: u8) -> Hash28 {
        let mut bytes = [0u8; 28];
        bytes[0] = b;
        Hash28::from_bytes(bytes)
    }

    /// Helper to create a sorted set of genesis keys from byte values.
    fn make_keys(bs: &[u8]) -> BTreeSet<Hash28> {
        bs.iter().map(|&b| make_key(b)).collect()
    }

    // ---- is_overlay_slot tests ----

    #[test]
    fn test_is_overlay_slot_d_zero() {
        // d = 0 means no overlay slots at all (fully decentralized)
        for slot in 0..20 {
            assert!(
                !is_overlay_slot(0, 0, 1, slot),
                "slot {slot} should not be overlay when d=0"
            );
        }
    }

    #[test]
    fn test_is_overlay_slot_d_one() {
        // d = 1 means all slots are overlay (fully federated)
        for slot in 0..20 {
            assert!(
                is_overlay_slot(0, 1, 1, slot),
                "slot {slot} should be overlay when d=1"
            );
        }
    }

    #[test]
    fn test_is_overlay_slot_d_half() {
        // d = 1/2 means every other slot is overlay
        // ceil(s * 1/2) < ceil((s+1) * 1/2)
        // s=0: ceil(0) < ceil(1/2) => 0 < 1 => true
        // s=1: ceil(1/2) < ceil(1) => 1 < 1 => false
        // s=2: ceil(1) < ceil(3/2) => 1 < 2 => true
        // s=3: ceil(3/2) < ceil(2) => 2 < 2 => false
        // Pattern: overlay at even offsets (0, 2, 4, 6, 8)
        let overlay_slots: Vec<u64> = (0..10).filter(|&s| is_overlay_slot(0, 1, 2, s)).collect();
        assert_eq!(overlay_slots, vec![0, 2, 4, 6, 8]);
    }

    #[test]
    fn test_is_overlay_slot_d_one_fifth() {
        // d = 1/5 means roughly every 5th slot
        // s=0: ceil(0) < ceil(1/5) => 0 < 1 => true
        // s=1: ceil(1/5) < ceil(2/5) => 1 < 1 => false
        // s=4: ceil(4/5) < ceil(1) => 1 < 1 => false
        // s=5: ceil(1) < ceil(6/5) => 1 < 2 => true
        let overlay_slots: Vec<u64> = (0..20).filter(|&s| is_overlay_slot(0, 1, 5, s)).collect();
        assert_eq!(overlay_slots, vec![0, 5, 10, 15]);
    }

    #[test]
    fn test_is_overlay_slot_with_offset() {
        // first_slot=100, d=1/2: overlay at offsets 0, 2, 4, ...
        // So slots 100, 102, 104, 106, 108
        let overlay_slots: Vec<u64> = (100..110)
            .filter(|&s| is_overlay_slot(100, 1, 2, s))
            .collect();
        assert_eq!(overlay_slots, vec![100, 102, 104, 106, 108]);
    }

    #[test]
    fn test_is_overlay_slot_mainnet_d() {
        // d = 9/10 means 90 out of 100 slots are overlay
        let count = (0..100).filter(|&s| is_overlay_slot(0, 9, 10, s)).count();
        assert_eq!(count, 90);
    }

    // ---- ceiling_mul tests ----

    #[test]
    fn test_ceiling_mul_exact() {
        // Exact division: ceil(6 * 2 / 3) = ceil(4) = 4
        assert_eq!(ceiling_mul(6, 2, 3), 4);
        // Non-exact: ceil(5 * 1 / 3) = ceil(5/3) = 2
        assert_eq!(ceiling_mul(5, 1, 3), 2);
        // Zero: ceil(0 * 1 / 3) = 0
        assert_eq!(ceiling_mul(0, 1, 3), 0);
        // Exact: ceil(10 * 1 / 5) = 2
        assert_eq!(ceiling_mul(10, 1, 5), 2);
        // Non-exact: ceil(7 * 1 / 2) = ceil(3.5) = 4
        assert_eq!(ceiling_mul(7, 1, 2), 4);
        // ceil(1 * 1 / 1) = 1
        assert_eq!(ceiling_mul(1, 1, 1), 1);
    }

    // ---- classify_overlay_slot tests ----

    #[test]
    fn test_classify_overlay_slot_active() {
        // d=1, f=1/20, 3 keys: active at positions 0, 20, 40, 60
        // asc_inv = floor(1 / (1/20)) = 20
        // position = ceil(offset * 1) = offset
        // active when position % 20 == 0 => slots 0, 20, 40, 60
        // key_index = (position / 20) % 3
        let keys = make_keys(&[1, 2, 3]);

        // Slot 0: position=0, 0%20==0, index=(0/20)%3=0 => key 1
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 0),
            OBftSlot::ActiveSlot(make_key(1))
        );
        // Slot 20: position=20, 20%20==0, index=(20/20)%3=1 => key 2
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 20),
            OBftSlot::ActiveSlot(make_key(2))
        );
        // Slot 40: position=40, 40%20==0, index=(40/20)%3=2 => key 3
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 40),
            OBftSlot::ActiveSlot(make_key(3))
        );
        // Slot 60: position=60, 60%20==0, index=(60/20)%3=0 => key 1 (wraps)
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 60),
            OBftSlot::ActiveSlot(make_key(1))
        );
    }

    #[test]
    fn test_classify_overlay_slot_non_active() {
        // d=1, f=1/20, asc_inv=20: non-active when position % 20 != 0
        let keys = make_keys(&[1, 2, 3]);

        // Slot 1: position=1, 1%20!=0 => NonActiveSlot
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 1),
            OBftSlot::NonActiveSlot
        );
        // Slot 19: position=19, 19%20!=0 => NonActiveSlot
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 19),
            OBftSlot::NonActiveSlot
        );
    }

    // ---- lookup_in_overlay_schedule tests ----

    #[test]
    fn test_lookup_praos_slot() {
        // d=1/2, slot 1 is not an overlay slot => None (Praos)
        let keys = make_keys(&[1, 2, 3]);
        assert_eq!(lookup_in_overlay_schedule(0, &keys, 1, 2, 1, 20, 1), None);
    }

    #[test]
    fn test_lookup_overlay_active() {
        // d=1, f=1/20, slot 0 => Some(ActiveSlot)
        let keys = make_keys(&[1, 2, 3]);
        assert_eq!(
            lookup_in_overlay_schedule(0, &keys, 1, 1, 1, 20, 0),
            Some(OBftSlot::ActiveSlot(make_key(1)))
        );
    }

    #[test]
    fn test_lookup_overlay_non_active() {
        // d=1, f=1/20, slot 1 => Some(NonActiveSlot)
        let keys = make_keys(&[1, 2, 3]);
        assert_eq!(
            lookup_in_overlay_schedule(0, &keys, 1, 1, 1, 20, 1),
            Some(OBftSlot::NonActiveSlot)
        );
    }

    #[test]
    fn test_classify_with_epoch_offset() {
        // first_slot=100, d=1, f=1/20, 3 keys
        // Slot 100: offset=0, position=0, active, index=0 => key 1
        // Slot 120: offset=20, position=20, active, index=1 => key 2
        let keys = make_keys(&[1, 2, 3]);

        assert_eq!(
            classify_overlay_slot(100, &keys, 1, 1, 1, 20, 100),
            OBftSlot::ActiveSlot(make_key(1))
        );
        assert_eq!(
            classify_overlay_slot(100, &keys, 1, 1, 1, 20, 120),
            OBftSlot::ActiveSlot(make_key(2))
        );
    }

    #[test]
    fn test_classify_empty_genesis_keys() {
        // Empty genesis key set => always NonActiveSlot
        let keys = BTreeSet::new();
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 0),
            OBftSlot::NonActiveSlot
        );
    }

    #[test]
    fn test_overlay_preview_params() {
        // Preview network-like params: f=1/20, d=1, 7 genesis keys
        // Active every 20th slot, round-robin across 7 keys
        let keys = make_keys(&[1, 2, 3, 4, 5, 6, 7]);

        let mut active_slots = Vec::new();
        for slot in 0..200 {
            if let OBftSlot::ActiveSlot(_) = classify_overlay_slot(0, &keys, 1, 1, 1, 20, slot) {
                active_slots.push(slot);
            }
        }
        // Should be active at 0, 20, 40, 60, 80, 100, 120, 140, 160, 180
        let expected: Vec<u64> = (0..200).step_by(20).collect();
        assert_eq!(active_slots, expected);

        // Verify round-robin: after 7 active slots (0..140 step 20), wraps back
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 0),
            OBftSlot::ActiveSlot(make_key(1))
        );
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 20, 140),
            OBftSlot::ActiveSlot(make_key(1)) // (140/20)%7 = 7%7 = 0
        );
    }

    #[test]
    fn test_overlay_context_full_workflow() {
        // Create an OverlayContext and verify delegate lookup through it
        let key_a = make_key(0x0A);
        let key_b = make_key(0x0B);

        let delegate_a = make_key(0xDA);
        let delegate_b = make_key(0xDB);
        let vrf_a = Hash32::from_bytes([0xA0; 32]);
        let vrf_b = Hash32::from_bytes([0xB0; 32]);

        let mut genesis_delegates = HashMap::new();
        genesis_delegates.insert(key_a, (delegate_a, vrf_a));
        genesis_delegates.insert(key_b, (delegate_b, vrf_b));

        let mut genesis_keys = BTreeSet::new();
        genesis_keys.insert(key_a);
        genesis_keys.insert(key_b);

        let ctx = OverlayContext {
            genesis_delegates,
            genesis_keys: genesis_keys.clone(),
            d: (1, 1),
            first_slot_of_epoch: 0,
        };

        // Slot 0 should be active with first key in sorted order
        let result = lookup_in_overlay_schedule(
            ctx.first_slot_of_epoch,
            &ctx.genesis_keys,
            ctx.d.0,
            ctx.d.1,
            1,
            20,
            0,
        );

        if let Some(OBftSlot::ActiveSlot(gkey)) = result {
            // Verify we can look up the delegate
            let (delegate, vrf) = ctx.genesis_delegates.get(&gkey).unwrap();
            assert_ne!(*delegate, gkey); // delegate differs from genesis key
            assert_ne!(*vrf, Hash32::ZERO);
        } else {
            panic!("Expected ActiveSlot at slot 0");
        }
    }

    #[test]
    fn test_overlay_d_transition() {
        // Verify overlay slot counts at different d values
        let range = 0..100u64;

        // d=1: all 100 slots are overlay
        let count_d1 = range.clone().filter(|&s| is_overlay_slot(0, 1, 1, s)).count();
        assert_eq!(count_d1, 100);

        // d=1/2: 50 overlay slots
        let count_d_half = range.clone().filter(|&s| is_overlay_slot(0, 1, 2, s)).count();
        assert_eq!(count_d_half, 50);

        // d=0: no overlay slots
        let count_d0 = range.filter(|&s| is_overlay_slot(0, 0, 1, s)).count();
        assert_eq!(count_d0, 0);
    }

    #[test]
    fn test_overlay_genesis_key_round_robin() {
        // Verify that genesis keys are assigned in sorted (BTreeSet) order
        let keys = make_keys(&[0x30, 0x10, 0x20]); // BTreeSet sorts: 0x10, 0x20, 0x30

        // d=1, f=1/1 (asc_inv=1, every slot is active)
        // Slot 0: index=0%3=0 => 0x10
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 1, 0),
            OBftSlot::ActiveSlot(make_key(0x10))
        );
        // Slot 1: index=1%3=1 => 0x20
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 1, 1),
            OBftSlot::ActiveSlot(make_key(0x20))
        );
        // Slot 2: index=2%3=2 => 0x30
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 1, 2),
            OBftSlot::ActiveSlot(make_key(0x30))
        );
        // Slot 3: index=3%3=0 => 0x10 (wraps)
        assert_eq!(
            classify_overlay_slot(0, &keys, 1, 1, 1, 1, 3),
            OBftSlot::ActiveSlot(make_key(0x10))
        );
    }
}
