use torsten_primitives::hash::Hash32;
use torsten_primitives::time::SlotNo;

// Slot leader check
//
// A stake pool is elected as slot leader if its VRF output for the slot
// satisfies: vrf_output < threshold(stake, f)
//
// The threshold function is: phi_f(sigma) = 1 - (1-f)^sigma
// where f is the active slot coefficient and sigma is relative stake.

/// Check if a pool is the slot leader for a given slot.
pub fn is_slot_leader(vrf_output: &[u8], relative_stake: f64, active_slot_coeff: f64) -> bool {
    torsten_crypto::vrf::check_leader_value(vrf_output, relative_stake, active_slot_coeff)
}

/// Compute the VRF input for a given slot
///
/// VRF input = hash(epoch_nonce || slot_number)
pub fn vrf_input(epoch_nonce: &Hash32, slot: SlotNo) -> Vec<u8> {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(epoch_nonce.as_bytes());
    data.extend_from_slice(&slot.0.to_be_bytes());
    data
}

/// Expected number of blocks per epoch
pub fn expected_blocks_per_epoch(epoch_length: u64, active_slot_coeff: f64) -> f64 {
    epoch_length as f64 * active_slot_coeff
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vrf_input() {
        let nonce = Hash32::from_bytes([1u8; 32]);
        let input = vrf_input(&nonce, SlotNo(100));
        assert_eq!(input.len(), 40); // 32 bytes nonce + 8 bytes slot
    }

    #[test]
    fn test_expected_blocks() {
        let expected = expected_blocks_per_epoch(432000, 0.05);
        assert!((expected - 21600.0).abs() < 0.1);
    }

    #[test]
    fn test_full_stake_leader() {
        // Pool with 100% stake and low VRF output should be leader
        assert!(is_slot_leader(&[0u8; 32], 1.0, 0.05));
    }

    #[test]
    fn test_zero_stake_not_leader() {
        assert!(!is_slot_leader(&[128u8; 32], 0.0, 0.05));
    }
}
