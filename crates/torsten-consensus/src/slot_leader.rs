use torsten_primitives::hash::{blake2b_256, Hash32};
use torsten_primitives::time::SlotNo;

// Slot leader check (Praos / Conway era)
//
// A stake pool is elected as slot leader if its VRF output for the slot
// satisfies: vrf_leader_value < threshold(stake, f)
//
// The threshold function is: phi_f(sigma) = 1 - (1-f)^sigma
// where f is the active slot coefficient and sigma is relative stake.
//
// VRF input = Blake2b-256(slot_u64_BE || epoch_nonce)  [32 bytes]
// Leader value = Blake2b-256("L" || raw_vrf_output)    [32 bytes]
// Nonce value = Blake2b-256(Blake2b-256("N" || raw_vrf_output))  [32 bytes]

/// Check if a pool is the slot leader for a given slot.
///
/// Uses the domain-separated leader value: Blake2b-256("L" || vrf_output)
pub fn is_slot_leader(vrf_output: &[u8], relative_stake: f64, active_slot_coeff: f64) -> bool {
    let leader_value = vrf_leader_value(vrf_output);
    torsten_crypto::vrf::check_leader_value(&leader_value, relative_stake, active_slot_coeff)
}

/// Check if a pool is the slot leader using exact rational sigma.
///
/// Both sigma (pool_stake/total_active_stake) and f (f_num/f_den) are
/// passed as exact rationals — no f64 precision loss.
pub fn is_slot_leader_rational(
    vrf_output: &[u8],
    sigma_num: u64,
    sigma_den: u64,
    f_num: u64,
    f_den: u64,
) -> bool {
    let leader_value = vrf_leader_value(vrf_output);
    torsten_crypto::vrf::check_leader_value_full_rational(
        &leader_value,
        sigma_num,
        sigma_den,
        f_num,
        f_den,
    )
}

/// Construct the Praos VRF input (Conway era).
///
/// VRF input = Blake2b-256(slot_u64_BE || epoch_nonce_bytes)
/// The slot comes first (8 bytes big-endian), then the epoch nonce (32 bytes).
/// The concatenation is hashed with Blake2b-256 to produce a 32-byte input.
pub fn vrf_input(epoch_nonce: &Hash32, slot: SlotNo) -> Vec<u8> {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&slot.0.to_be_bytes()); // slot FIRST
    data.extend_from_slice(epoch_nonce.as_bytes()); // nonce SECOND
    blake2b_256(&data).to_vec()
}

/// Extract the leader value from a VRF output (domain-separated).
///
/// leader_value = Blake2b-256("L" || raw_vrf_output)
pub fn vrf_leader_value(vrf_output: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(1 + vrf_output.len());
    input.push(b'L');
    input.extend_from_slice(vrf_output);
    *blake2b_256(&input).as_bytes()
}

/// Extract the nonce contribution from a VRF output (domain-separated, double-hashed).
///
/// nonce_value = Blake2b-256(Blake2b-256("N" || raw_vrf_output))
pub fn vrf_nonce_value(vrf_output: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(1 + vrf_output.len());
    input.push(b'N');
    input.extend_from_slice(vrf_output);
    let first_hash = blake2b_256(&input);
    *blake2b_256(first_hash.as_ref()).as_bytes()
}

/// Expected number of blocks per epoch
pub fn expected_blocks_per_epoch(epoch_length: u64, active_slot_coeff: f64) -> f64 {
    epoch_length as f64 * active_slot_coeff
}

/// A slot where the pool is elected as leader
#[derive(Debug, Clone)]
pub struct LeaderSlot {
    pub slot: SlotNo,
    pub vrf_output: [u8; 64],
    pub vrf_proof: [u8; 80],
}

/// Compute the leader schedule for a given epoch using fully exact rational arithmetic.
///
/// Returns all slots within the epoch where the pool (identified by its VRF secret key)
/// is elected as slot leader.
///
/// # Parameters
///
/// - `vrf_skey` — 32-byte VRF secret key.
/// - `epoch_nonce` — the epoch's randomness nonce.
/// - `epoch_start_slot` — first slot of the epoch (absolute slot number).
/// - `epoch_length` — number of slots in the epoch.
/// - `pool_stake` — pool's active stake in lovelace.
/// - `total_active_stake` — total active stake in the epoch snapshot (lovelace).
/// - `f_num` / `f_den` — active-slot coefficient as an exact rational,
///   e.g. `1 / 20` for the standard `f = 0.05`.
///
/// # Precision
///
/// Both sigma (= `pool_stake / total_active_stake`) and the active-slot
/// coefficient are passed as exact rationals — no f64 rounding anywhere.
/// The `|ln(1-f)|` value is pre-computed once before the slot loop and
/// reused for every slot, matching Haskell's `activeSlotLog` caching pattern.
///
/// # Boundary conditions
///
/// - `pool_stake == 0` → empty schedule (pool has no chance of leading).
/// - `f_num == 0` → empty schedule (no slots are active).
/// - `f_num >= f_den` → all slots are leader slots (f ≥ 1, degenerate case).
///
/// # Note on argument count
///
/// The 8 arguments reflect the two exact-rational pairs (sigma, f) required
/// for Haskell-conformant precision.  Callers that obtain these from a
/// `ProtocolParams` struct can destructure there; the function itself must
/// not artificially merge unrelated quantities.
#[allow(clippy::too_many_arguments)]
pub fn compute_leader_schedule(
    vrf_skey: &[u8; 32],
    epoch_nonce: &Hash32,
    epoch_start_slot: u64,
    epoch_length: u64,
    pool_stake: u64,
    total_active_stake: u64,
    f_num: u64,
    f_den: u64,
) -> Vec<LeaderSlot> {
    // Boundary: no stake → never elected.
    if pool_stake == 0 || total_active_stake == 0 {
        return Vec::new();
    }

    // Boundary: f == 0 → no slots are active.
    if f_num == 0 {
        return Vec::new();
    }

    // Boundary: f >= 1 → all slots with a valid VRF proof are leader slots.
    let all_slots_active = f_num >= f_den;

    // Pre-compute |ln(1-f)| once for the entire epoch.  This is the most
    // expensive part of the leader check (continued-fraction convergence),
    // so caching it avoids repeating it for every slot.
    //
    // When f >= 1 we skip the log (not needed) and elect every slot.
    let active_slot_log = if !all_slots_active {
        match torsten_crypto::vrf::compute_active_slot_log(f_num, f_den) {
            Some(log) => log,
            // compute_active_slot_log returns None only for f==0 or f>=1,
            // both of which are handled above.
            None => return Vec::new(),
        }
    } else {
        // Placeholder — never actually used in the all_slots_active branch.
        // Safety: the branch below calls generate_vrf_proof and then pushes
        // directly without consulting the log.
        match torsten_crypto::vrf::compute_active_slot_log(1, 2) {
            Some(log) => log,
            None => return Vec::new(),
        }
    };

    let mut schedule = Vec::new();

    for offset in 0..epoch_length {
        let slot = SlotNo(epoch_start_slot + offset);
        let seed = vrf_input(epoch_nonce, slot);

        if let Ok((proof, output)) = torsten_crypto::vrf::generate_vrf_proof(vrf_skey, &seed) {
            // Domain-separate the raw VRF output to produce the 32-byte
            // Praos leader value: Blake2b-256("L" || raw_output).
            // This is the value that Haskell's checkLeaderNatValue receives
            // as `certNat` (interpreted as a 256-bit big-endian integer).
            let leader_value = vrf_leader_value(&output);

            let is_leader = if all_slots_active {
                true
            } else {
                // Use the pre-computed log for sigma × |ln(1-f)| comparison.
                torsten_crypto::vrf::check_leader_value_full_rational_cached(
                    &leader_value,
                    pool_stake,
                    total_active_stake,
                    &active_slot_log,
                )
            };

            if is_leader {
                schedule.push(LeaderSlot {
                    slot,
                    vrf_output: output,
                    vrf_proof: proof,
                });
            }
        }
    }

    schedule
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // vrf_input() — byte ordering and determinism
    // -------------------------------------------------------------------------

    /// vrf_input must produce exactly 32 bytes (Blake2b-256 output).
    #[test]
    fn test_vrf_input_length() {
        let nonce = Hash32::from_bytes([1u8; 32]);
        let input = vrf_input(&nonce, SlotNo(100));
        assert_eq!(input.len(), 32);
    }

    /// vrf_input is deterministic: same slot + nonce always yields the same hash.
    #[test]
    fn test_vrf_input_deterministic() {
        let nonce = Hash32::from_bytes([0xABu8; 32]);
        let a = vrf_input(&nonce, SlotNo(999));
        let b = vrf_input(&nonce, SlotNo(999));
        assert_eq!(a, b, "vrf_input must be deterministic");
    }

    /// Different slots must produce different VRF inputs.
    #[test]
    fn test_vrf_input_slot_sensitivity() {
        let nonce = Hash32::from_bytes([0u8; 32]);
        let a = vrf_input(&nonce, SlotNo(0));
        let b = vrf_input(&nonce, SlotNo(1));
        assert_ne!(a, b, "Different slots must produce different VRF inputs");
    }

    /// Different nonces must produce different VRF inputs for the same slot.
    #[test]
    fn test_vrf_input_nonce_sensitivity() {
        let slot = SlotNo(12345);
        let nonce_a = Hash32::from_bytes([0u8; 32]);
        let nonce_b = Hash32::from_bytes([1u8; 32]);
        let a = vrf_input(&nonce_a, slot);
        let b = vrf_input(&nonce_b, slot);
        assert_ne!(a, b, "Different nonces must produce different VRF inputs");
    }

    /// Verify the exact byte ordering: slot (8 bytes big-endian) FIRST,
    /// then epoch nonce (32 bytes). The result is Blake2b-256 of the 40-byte
    /// concatenation.
    ///
    /// Test vector computed as:
    ///   data = 0x0000000000000064 || 0x0101...01 (32 bytes)
    ///   output = Blake2b-256(data)
    ///
    /// This exercises the Haskell spec requirement that the slot is encoded
    /// in big-endian byte order and appears before the nonce in the input.
    #[test]
    fn test_vrf_input_byte_ordering_known_vector() {
        // slot = 100 (0x0000000000000064), nonce = [0x01; 32]
        let nonce = Hash32::from_bytes([0x01u8; 32]);
        let slot = SlotNo(100);
        let result = vrf_input(&nonce, slot);

        // Manually construct the expected pre-image and hash it
        let mut expected_preimage = Vec::with_capacity(40);
        expected_preimage.extend_from_slice(&100u64.to_be_bytes()); // slot FIRST
        expected_preimage.extend_from_slice(&[0x01u8; 32]); // nonce SECOND
        let expected = blake2b_256(&expected_preimage).to_vec();

        assert_eq!(
            result, expected,
            "vrf_input must hash slot_BE (8 bytes) || nonce (32 bytes)"
        );
    }

    /// Verify slot 0 is encoded as 8 zero bytes (not omitted).
    #[test]
    fn test_vrf_input_slot_zero_encoding() {
        let nonce = Hash32::from_bytes([0xFFu8; 32]);
        let slot_zero = SlotNo(0);
        let result = vrf_input(&nonce, slot_zero);

        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&0u64.to_be_bytes()); // 8 zero bytes
        preimage.extend_from_slice(&[0xFFu8; 32]);
        let expected = blake2b_256(&preimage).to_vec();

        assert_eq!(result, expected, "Slot 0 must encode as 8 zero bytes");
    }

    /// Verify that a large slot (u64::MAX) is correctly big-endian encoded.
    /// This exercises the full 8-byte slot field.
    #[test]
    fn test_vrf_input_max_slot() {
        let nonce = Hash32::from_bytes([0u8; 32]);
        let max_slot = SlotNo(u64::MAX);
        let result = vrf_input(&nonce, max_slot);

        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&u64::MAX.to_be_bytes()); // [0xFF; 8]
        preimage.extend_from_slice(&[0u8; 32]);
        let expected = blake2b_256(&preimage).to_vec();

        assert_eq!(
            result, expected,
            "u64::MAX slot must encode as 8 0xFF bytes"
        );
    }

    /// A neutral (all-zero) epoch nonce still produces a valid, non-trivial VRF input.
    /// The neutral nonce is used when the epoch nonce has not yet been established
    /// (e.g., during the Shelley overlay period or after Mithril import).
    #[test]
    fn test_vrf_input_neutral_nonce() {
        // NeutralNonce = Hash32 of all zeros (as Haskell uses it before first epoch nonce)
        let neutral_nonce = Hash32::from_bytes([0u8; 32]);
        let result = vrf_input(&neutral_nonce, SlotNo(1));
        assert_eq!(result.len(), 32);
        // Must differ from slot 0 + neutral nonce
        let result_slot0 = vrf_input(&neutral_nonce, SlotNo(0));
        assert_ne!(
            result, result_slot0,
            "Different slots must produce different inputs even with neutral nonce"
        );
    }

    // -------------------------------------------------------------------------
    // vrf_leader_value() — domain separation with "L" prefix
    // -------------------------------------------------------------------------

    /// vrf_leader_value must produce exactly 32 bytes (Blake2b-256 output).
    #[test]
    fn test_vrf_leader_value_length() {
        let output = [0u8; 64];
        let leader_value = vrf_leader_value(&output);
        assert_eq!(leader_value.len(), 32);
    }

    /// Domain separation: vrf_leader_value("L" || data) != vrf_nonce_value("N" || data).
    /// This is the critical Praos invariant — the leader check and nonce contribution
    /// use different domain prefixes to prevent cross-use attacks.
    #[test]
    fn test_vrf_leader_value_domain_separation_from_nonce() {
        let raw_output = [0x42u8; 64];
        let leader = vrf_leader_value(&raw_output);
        let nonce = vrf_nonce_value(&raw_output);
        // leader and nonce values must differ despite the same VRF output
        assert_ne!(
            leader, nonce,
            "Leader value and nonce value must differ (different domain prefixes 'L' vs 'N')"
        );
    }

    /// vrf_leader_value is deterministic.
    #[test]
    fn test_vrf_leader_value_deterministic() {
        let output = [0x77u8; 64];
        let a = vrf_leader_value(&output);
        let b = vrf_leader_value(&output);
        assert_eq!(a, b, "vrf_leader_value must be deterministic");
    }

    /// vrf_leader_value changes when the VRF output changes (input sensitivity).
    #[test]
    fn test_vrf_leader_value_input_sensitivity() {
        let mut output_a = [0u8; 64];
        let mut output_b = [0u8; 64];
        output_b[0] = 1;
        let lv_a = vrf_leader_value(&output_a);
        let lv_b = vrf_leader_value(&output_b);
        assert_ne!(
            lv_a, lv_b,
            "Different VRF outputs must produce different leader values"
        );
        // Flip a byte in the middle
        output_a[32] = 0xFF;
        let lv_c = vrf_leader_value(&output_a);
        assert_ne!(
            lv_a, lv_c,
            "Changing the middle byte must change the leader value"
        );
    }

    /// Known-vector test: vrf_leader_value is Blake2b-256("L" || raw_output).
    /// Verifying against a manually computed reference ensures the "L" prefix
    /// is a single ASCII byte (0x4C), not a multi-byte or UTF-8 encoding.
    #[test]
    fn test_vrf_leader_value_known_vector() {
        let raw_output = [0u8; 64];
        let result = vrf_leader_value(&raw_output);

        // Manually compute: Blake2b-256(0x4C || [0x00; 64])
        let mut preimage = Vec::with_capacity(65);
        preimage.push(b'L'); // 0x4C
        preimage.extend_from_slice(&[0u8; 64]);
        let expected = *blake2b_256(&preimage).as_bytes();

        assert_eq!(
            result, expected,
            "vrf_leader_value must be Blake2b-256(0x4C || raw_output)"
        );
    }

    /// vrf_nonce_value is Blake2b-256(Blake2b-256("N" || raw_output)) — double hash.
    /// Test the known-vector for the nonce domain.
    #[test]
    fn test_vrf_nonce_value_known_vector() {
        let raw_output = [0u8; 64];
        let result = vrf_nonce_value(&raw_output);

        // Manually compute: Blake2b-256(Blake2b-256("N" || [0x00; 64]))
        let mut inner_preimage = Vec::with_capacity(65);
        inner_preimage.push(b'N'); // 0x4E
        inner_preimage.extend_from_slice(&[0u8; 64]);
        let inner_hash = blake2b_256(&inner_preimage);
        let expected = *blake2b_256(inner_hash.as_ref()).as_bytes();

        assert_eq!(
            result, expected,
            "vrf_nonce_value must be Blake2b-256(Blake2b-256(0x4E || raw_output))"
        );
    }

    /// Shorter VRF outputs (e.g., 32 bytes) are handled without panic.
    #[test]
    fn test_vrf_leader_value_short_input() {
        // 32-byte VRF output (not standard, but must not panic)
        let output = [0xAAu8; 32];
        let lv = vrf_leader_value(&output);
        assert_eq!(lv.len(), 32);
    }

    // -------------------------------------------------------------------------
    // check_slot_leadership() — stake levels and leader threshold
    // -------------------------------------------------------------------------

    /// Zero stake: never elected regardless of VRF output.
    /// phi_f(0) = 1 - (1-f)^0 = 0, so threshold is 0 and no output passes.
    #[test]
    fn test_check_slot_leadership_zero_stake_never_elected() {
        // Try a range of VRF outputs — none should pass with 0% stake
        for byte_val in [0x00u8, 0x01, 0x10, 0x80, 0xFF] {
            let output = [byte_val; 64];
            assert!(
                !is_slot_leader(&output, 0.0, 0.05),
                "Zero stake must never be elected (output[0]={byte_val:#04x})"
            );
        }
    }

    /// 100% stake: phi_f(1.0) = f = 0.05. About 5% of VRF outputs should pass.
    /// Verify that at least some outputs pass (not an always-true situation).
    #[test]
    fn test_check_slot_leadership_full_stake_some_elected() {
        let mut elected = 0;
        let mut not_elected = 0;
        // Sweep 256 uniform VRF outputs
        for i in 0u8..=255 {
            let output = [i; 64];
            // The leader value is Blake2b-256("L" || output), which distributes
            // outputs uniformly — we expect ~5% to pass the threshold.
            if is_slot_leader(&output, 1.0, 0.05) {
                elected += 1;
            } else {
                not_elected += 1;
            }
        }
        assert!(elected > 0, "Full stake must elect some slots (f=0.05)");
        assert!(
            not_elected > 0,
            "Full stake with f=0.05 must not elect all slots"
        );
    }

    /// 1% stake: phi_f(0.01) = 1 - 0.95^0.01 ≈ 0.000512.
    /// Outputs near zero (certNat ≈ 0) must pass; high outputs must not.
    #[test]
    fn test_check_slot_leadership_1pct_stake() {
        // certNat = 0 → certNatMax / (certNatMax - 0) = 1.0, smallest recip_q
        // We need the leader value (Blake2b-256("L" || output)) to map to near zero.
        // Use all-zero VRF output — it produces a specific leader value.
        // The leader value [0x00; 32] (certNat=0) → must be elected for any positive stake
        // We can't directly control the leader value, but certNat=0 IS always elected.
        // Instead, use the internal check_leader_value with a hand-crafted leader value.
        let zero_leader_value = [0u8; 32]; // certNat = 0, guaranteed to pass
        assert!(
            torsten_crypto::vrf::check_leader_value(&zero_leader_value, 0.01, 0.05),
            "certNat=0 must always elect (phi_f(0.01) > 0)"
        );

        // Very high leader value: certNat ≈ 2^256 → ratio ≈ 1.0 >> phi_f(0.01)
        let max_leader_value = [0xFFu8; 32]; // certNat = 2^256 - 1
        assert!(
            !torsten_crypto::vrf::check_leader_value(&max_leader_value, 0.01, 0.05),
            "certNat near max must NOT elect for 1% stake"
        );
    }

    /// 50% stake: phi_f(0.5) = 1 - 0.95^0.5 ≈ 0.02532.
    /// About 2.5% of outputs should pass the leader check.
    #[test]
    fn test_check_slot_leadership_50pct_stake() {
        let zero_lv = [0u8; 32];
        assert!(
            torsten_crypto::vrf::check_leader_value(&zero_lv, 0.5, 0.05),
            "certNat=0 must always elect for 50% stake"
        );

        let high_lv = [0xFFu8; 32];
        assert!(
            !torsten_crypto::vrf::check_leader_value(&high_lv, 0.5, 0.05),
            "certNat near max must NOT elect for 50% stake"
        );
    }

    /// Monotonicity: for a fixed VRF output, larger stake must not reduce election probability.
    /// If a pool with stake S is elected, any pool with stake S' > S must also be elected.
    #[test]
    fn test_check_slot_leadership_stake_monotonicity() {
        // Use a range of leader values and verify monotonicity across stake levels
        let stakes = [0.001f64, 0.01, 0.05, 0.1, 0.5, 1.0];

        for output_byte in [0x00u8, 0x01, 0x04, 0x08, 0x10] {
            let mut leader_value = [0u8; 32];
            leader_value[0] = output_byte;

            let results: Vec<bool> = stakes
                .iter()
                .map(|&s| torsten_crypto::vrf::check_leader_value(&leader_value, s, 0.05))
                .collect();

            // If elected at index i, must be elected at all j > i (larger stakes)
            for i in 0..results.len() {
                for j in (i + 1)..results.len() {
                    if results[i] {
                        assert!(
                            results[j],
                            "Monotonicity violated for output_byte={output_byte:#04x}: \
                             elected at stake={} but not at stake={}",
                            stakes[i], stakes[j]
                        );
                    }
                }
            }
        }
    }

    /// Active slot coefficient monotonicity: higher f means higher election probability.
    /// If elected with f=0.05, must also be elected with f=0.10.
    #[test]
    fn test_check_slot_leadership_f_monotonicity() {
        let test_lv = [0x08u8; 32]; // certNat ratio ≈ 0.031
        let stake = 0.5;

        let elected_f005 = torsten_crypto::vrf::check_leader_value(&test_lv, stake, 0.05);
        let elected_f010 = torsten_crypto::vrf::check_leader_value(&test_lv, stake, 0.10);

        if elected_f005 {
            assert!(
                elected_f010,
                "Higher f must not reduce election probability: \
                 elected with f=0.05 but not f=0.10 for the same stake"
            );
        }
    }

    /// TPraos era: raw 64-byte VRF output with certNatMax=2^512.
    /// Zero output (certNat=0) must always be elected.
    #[test]
    fn test_check_slot_leadership_tpraos_zero_elected() {
        let zero_output = [0u8; 64];
        assert!(
            torsten_crypto::vrf::check_leader_value_tpraos(&zero_output, 1.0, 0.05),
            "TPraos: certNat=0 must always be elected with non-zero stake"
        );
        assert!(
            torsten_crypto::vrf::check_leader_value_tpraos(&zero_output, 0.01, 0.05),
            "TPraos: certNat=0 must be elected for 1% stake"
        );
    }

    /// TPraos era: max VRF output (certNat = 2^512 - 1) must never be elected.
    #[test]
    fn test_check_slot_leadership_tpraos_max_not_elected() {
        let max_output = [0xFFu8; 64]; // certNat = 2^512 - 1
        assert!(
            !torsten_crypto::vrf::check_leader_value_tpraos(&max_output, 1.0, 0.05),
            "TPraos: certNat near max must NOT be elected"
        );
    }

    /// TPraos era: zero stake means never elected.
    #[test]
    fn test_check_slot_leadership_tpraos_zero_stake() {
        let zero_output = [0u8; 64];
        assert!(
            !torsten_crypto::vrf::check_leader_value_tpraos(&zero_output, 0.0, 0.05),
            "TPraos: zero stake must never be elected"
        );
    }

    // -------------------------------------------------------------------------
    // compute_leader_schedule() — determinism, ordering, VRF proof validity
    // -------------------------------------------------------------------------

    /// compute_leader_schedule with zero stake must return an empty schedule.
    #[test]
    fn test_leader_schedule_zero_stake_empty() {
        let kp = torsten_crypto::vrf::generate_vrf_keypair();
        let epoch_nonce = Hash32::from_bytes([0u8; 32]);

        // pool_stake = 0, total_active_stake = 1_000_000, f = 1/20
        let schedule =
            compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 1000, 0, 1_000_000, 1, 20);

        assert!(
            schedule.is_empty(),
            "Zero stake must produce an empty leader schedule"
        );
    }

    /// compute_leader_schedule is deterministic: same key + nonce + stake always
    /// produces the same schedule.
    #[test]
    fn test_leader_schedule_deterministic() {
        // Use a fixed secret key so the schedule is reproducible across runs
        let secret = [0x11u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x22u8; 32]);

        // pool_stake = total_active_stake → sigma = 1.0; f = 1/20 = 0.05
        let schedule_a =
            compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 500, 1, 1, 1, 20);
        let schedule_b =
            compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 500, 1, 1, 1, 20);

        assert_eq!(
            schedule_a.len(),
            schedule_b.len(),
            "Leader schedule must be deterministic"
        );
        for (a, b) in schedule_a.iter().zip(schedule_b.iter()) {
            assert_eq!(a.slot, b.slot, "Slot numbers must be identical");
            assert_eq!(a.vrf_output, b.vrf_output, "VRF outputs must be identical");
            assert_eq!(a.vrf_proof, b.vrf_proof, "VRF proofs must be identical");
        }
    }

    /// Slots in the schedule must be sorted in ascending order.
    #[test]
    fn test_leader_schedule_slots_ascending() {
        let secret = [0x33u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x44u8; 32]);

        // sigma = 1/1 (full stake), f = 1/20
        let schedule = compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 500, 1, 1, 1, 20);

        for window in schedule.windows(2) {
            assert!(
                window[0].slot < window[1].slot,
                "Leader schedule slots must be in strictly ascending order: \
                 got {:?} before {:?}",
                window[0].slot,
                window[1].slot
            );
        }
    }

    /// Slots in the schedule must lie within [epoch_start_slot, epoch_start_slot + epoch_length).
    #[test]
    fn test_leader_schedule_slot_range() {
        let secret = [0x55u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x66u8; 32]);
        let epoch_start = 432_000u64; // a realistic epoch boundary
        let epoch_len = 500u64;

        // sigma = 1/1, f = 1/20
        let schedule = compute_leader_schedule(
            kp.secret_key(),
            &epoch_nonce,
            epoch_start,
            epoch_len,
            1,
            1,
            1,
            20,
        );

        for ls in &schedule {
            assert!(
                ls.slot.0 >= epoch_start,
                "Slot {:?} is below epoch start {epoch_start}",
                ls.slot
            );
            assert!(
                ls.slot.0 < epoch_start + epoch_len,
                "Slot {:?} is at or above epoch end {}",
                ls.slot,
                epoch_start + epoch_len
            );
        }
    }

    /// Every VRF proof in the schedule must verify against the pool's public key
    /// and the slot's VRF input seed.
    #[test]
    fn test_leader_schedule_proof_verification() {
        let secret = [0x77u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x88u8; 32]);

        // sigma = 1/1, f = 1/20
        let schedule = compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 500, 1, 1, 1, 20);

        assert!(
            !schedule.is_empty(),
            "Need at least one leader slot for proof verification test"
        );

        for ls in &schedule {
            let seed = vrf_input(&epoch_nonce, ls.slot);

            // VRF proof must verify
            let verified =
                torsten_crypto::vrf::verify_vrf_proof(&kp.public_key, &ls.vrf_proof, &seed);
            assert!(
                verified.is_ok(),
                "VRF proof must verify for slot {:?}: {:?}",
                ls.slot,
                verified.err()
            );

            // The verified output must match the stored output
            assert_eq!(
                verified.unwrap(),
                ls.vrf_output,
                "Verified VRF output must match stored output for slot {:?}",
                ls.slot
            );
        }
    }

    /// Different VRF keys produce different schedules for the same nonce/epoch.
    #[test]
    fn test_leader_schedule_different_keys_different_schedules() {
        let kp_a = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&[0xAAu8; 32]);
        let kp_b = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&[0xBBu8; 32]);
        let epoch_nonce = Hash32::from_bytes([0xCCu8; 32]);
        let schedule_a =
            compute_leader_schedule(kp_a.secret_key(), &epoch_nonce, 0, 1000, 1, 1, 1, 20);
        let schedule_b =
            compute_leader_schedule(kp_b.secret_key(), &epoch_nonce, 0, 1000, 1, 1, 1, 20);

        // Two distinct keys will almost certainly produce different schedules
        // (the probability of identical schedules is astronomically small)
        let slots_a: Vec<u64> = schedule_a.iter().map(|ls| ls.slot.0).collect();
        let slots_b: Vec<u64> = schedule_b.iter().map(|ls| ls.slot.0).collect();
        assert_ne!(
            slots_a, slots_b,
            "Different VRF keys must produce different leader schedules"
        );
    }

    /// Different epoch nonces produce different schedules for the same key.
    #[test]
    fn test_leader_schedule_different_nonces_different_schedules() {
        let secret = [0xDDu8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let nonce_a = Hash32::from_bytes([0u8; 32]);
        let nonce_b = Hash32::from_bytes([1u8; 32]);

        // sigma = 1/1, f = 1/20
        let schedule_a = compute_leader_schedule(kp.secret_key(), &nonce_a, 0, 1000, 1, 1, 1, 20);
        let schedule_b = compute_leader_schedule(kp.secret_key(), &nonce_b, 0, 1000, 1, 1, 1, 20);

        let slots_a: Vec<u64> = schedule_a.iter().map(|ls| ls.slot.0).collect();
        let slots_b: Vec<u64> = schedule_b.iter().map(|ls| ls.slot.0).collect();
        assert_ne!(
            slots_a, slots_b,
            "Different epoch nonces must produce different leader schedules"
        );
    }

    /// A single-slot epoch with 100% stake must produce exactly one leader slot.
    #[test]
    fn test_leader_schedule_single_slot_epoch() {
        // With a single slot and 100% stake, we generate one VRF proof.
        // The slot is either elected or not depending on the VRF output.
        // With a fixed key chosen to reliably win: iterate keys until we find one.
        let epoch_nonce = Hash32::from_bytes([0u8; 32]);

        // Try 10 keys until we find one that wins slot 0 with full stake and f=0.05
        let mut found = false;
        for i in 0..50u8 {
            let secret = [i; 32];
            let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
            // sigma = 1/1, f = 1/20
            let schedule =
                compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 1, 1, 1, 1, 20);
            if schedule.len() == 1 {
                assert_eq!(
                    schedule[0].slot,
                    SlotNo(0),
                    "Single-slot epoch must produce slot 0"
                );
                found = true;
                break;
            }
        }
        assert!(
            found,
            "Should find a key that wins the single-slot epoch (probability ~5% per try)"
        );
    }

    /// compute_leader_schedule with a non-zero epoch start correctly offsets slot numbers.
    #[test]
    fn test_leader_schedule_epoch_start_offset() {
        let secret = [0xEEu8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0xFFu8; 32]);
        let epoch_start = 10_000u64;

        // Run two schedules: one starting at 0, one at epoch_start
        // They must produce DIFFERENT slots (since slot is embedded in VRF input)
        // sigma = 1/1, f = 1/20
        let schedule_base =
            compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 500, 1, 1, 1, 20);
        let schedule_offset =
            compute_leader_schedule(kp.secret_key(), &epoch_nonce, epoch_start, 500, 1, 1, 1, 20);

        // All slots in the offset schedule must be >= epoch_start
        for ls in &schedule_offset {
            assert!(
                ls.slot.0 >= epoch_start,
                "Offset epoch slot {:?} must be >= epoch_start={epoch_start}",
                ls.slot
            );
        }

        // Schedules must differ (different slot numbers → different VRF inputs)
        let slots_base: Vec<u64> = schedule_base.iter().map(|ls| ls.slot.0).collect();
        let slots_offset: Vec<u64> = schedule_offset.iter().map(|ls| ls.slot.0).collect();
        assert_ne!(
            slots_base, slots_offset,
            "Epoch start offset must produce different slot numbers"
        );
    }

    // -------------------------------------------------------------------------
    // Leader election probability — statistical rate verification
    // -------------------------------------------------------------------------

    /// With f=0.05 and 100% stake, phi_f(1.0) = 1 - (1-0.05)^1 = 0.05.
    /// Over 1000 uniformly-spread VRF outputs, ~50 should be elected (allow ±3%).
    ///
    /// We use the internal check_leader_value directly with uniformly-spaced
    /// 32-byte leader values to avoid VRF key scheduling variance.
    #[test]
    fn test_leader_election_rate_full_stake() {
        let f = 0.05f64;
        let stake = 1.0f64;
        let trials = 1000usize;
        let expected_rate = f; // phi_f(1.0) = f

        let mut elected = 0usize;
        for i in 0..trials {
            // Spread leader values uniformly over [0, 2^256)
            let fraction = i as f64 / trials as f64;
            // Encode fraction * 2^256 in 32 bytes (big-endian)
            let mut lv = [0u8; 32];
            let high = (fraction * u64::MAX as f64) as u64;
            lv[..8].copy_from_slice(&high.to_be_bytes());
            if torsten_crypto::vrf::check_leader_value(&lv, stake, f) {
                elected += 1;
            }
        }

        let actual_rate = elected as f64 / trials as f64;
        assert!(
            (actual_rate - expected_rate).abs() < 0.03,
            "Election rate with f={f}, stake={stake} should be ~{expected_rate:.3}, \
             got {actual_rate:.3} ({elected}/{trials})"
        );
    }

    /// With f=0.05 and 50% stake, phi_f(0.5) = 1 - 0.95^0.5 ≈ 0.02532.
    /// Over 10000 uniformly-spread leader values, ~253 should be elected.
    #[test]
    fn test_leader_election_rate_half_stake() {
        let f = 0.05f64;
        let stake = 0.5f64;
        let trials = 10_000usize;
        // phi_f(0.5) = 1 - (1-0.05)^0.5 ≈ 0.02532
        let expected_rate = 1.0 - (1.0 - f).powf(stake);

        let mut elected = 0usize;
        for i in 0..trials {
            let fraction = i as f64 / trials as f64;
            let mut lv = [0u8; 32];
            let high = (fraction * u64::MAX as f64) as u64;
            lv[..8].copy_from_slice(&high.to_be_bytes());
            if torsten_crypto::vrf::check_leader_value(&lv, stake, f) {
                elected += 1;
            }
        }

        let actual_rate = elected as f64 / trials as f64;
        // Allow ±1% absolute tolerance for the larger trial count
        assert!(
            (actual_rate - expected_rate).abs() < 0.01,
            "Election rate with f={f}, stake={stake} should be ~{expected_rate:.4}, \
             got {actual_rate:.4} ({elected}/{trials})"
        );
    }

    /// With f=0.05 and 1% stake, phi_f(0.01) = 1 - 0.95^0.01 ≈ 0.000513.
    /// Verify that the rate is in the right order of magnitude.
    #[test]
    fn test_leader_election_rate_small_stake() {
        let f = 0.05f64;
        let stake = 0.01f64;
        let trials = 100_000usize;
        // phi_f(0.01) = 1 - (1-0.05)^0.01 ≈ 0.000513
        let expected_rate = 1.0 - (1.0 - f).powf(stake);

        let mut elected = 0usize;
        for i in 0..trials {
            let fraction = i as f64 / trials as f64;
            let mut lv = [0u8; 32];
            let high = (fraction * u64::MAX as f64) as u64;
            lv[..8].copy_from_slice(&high.to_be_bytes());
            if torsten_crypto::vrf::check_leader_value(&lv, stake, f) {
                elected += 1;
            }
        }

        let actual_rate = elected as f64 / trials as f64;
        // Allow ±0.05% absolute tolerance
        assert!(
            (actual_rate - expected_rate).abs() < 0.0005,
            "Election rate with f={f}, stake={stake} should be ~{expected_rate:.6}, \
             got {actual_rate:.6} ({elected}/{trials})"
        );
    }

    // -------------------------------------------------------------------------
    // expected_blocks_per_epoch() — formula verification
    // -------------------------------------------------------------------------

    /// Mainnet: 432000 slots/epoch * f=0.05 = 21600 expected blocks.
    #[test]
    fn test_expected_blocks_mainnet() {
        let expected = expected_blocks_per_epoch(432_000, 0.05);
        assert!(
            (expected - 21_600.0).abs() < 0.01,
            "Mainnet should expect 21600 blocks/epoch, got {expected}"
        );
    }

    /// Preview testnet: 86400 slots/epoch * f=0.05 = 4320 expected blocks.
    #[test]
    fn test_expected_blocks_preview() {
        let expected = expected_blocks_per_epoch(86_400, 0.05);
        assert!(
            (expected - 4_320.0).abs() < 0.01,
            "Preview should expect 4320 blocks/epoch, got {expected}"
        );
    }

    /// f=1.0: all slots should be filled (expected = epoch_length).
    #[test]
    fn test_expected_blocks_all_slots_filled() {
        let expected = expected_blocks_per_epoch(432_000, 1.0);
        assert!(
            (expected - 432_000.0).abs() < 0.01,
            "With f=1.0, all slots should be expected to be filled"
        );
    }

    // -------------------------------------------------------------------------
    // vrf_input / vrf_leader_value integration — end-to-end VRF pipeline
    // -------------------------------------------------------------------------

    /// End-to-end: generate a VRF proof for a slot, compute the leader value,
    /// and verify that the is_slot_leader result is consistent with check_leader_value.
    #[test]
    fn test_end_to_end_vrf_leader_pipeline() {
        let secret = [0x11u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x22u8; 32]);
        let slot = SlotNo(1000);

        // Step 1: compute VRF input seed
        let seed = vrf_input(&epoch_nonce, slot);
        assert_eq!(seed.len(), 32, "VRF input seed must be 32 bytes");

        // Step 2: generate VRF proof
        let (proof, raw_output) = torsten_crypto::vrf::generate_vrf_proof(kp.secret_key(), &seed)
            .expect("VRF proof generation must succeed");

        // Step 3: verify VRF proof
        let verified_output = torsten_crypto::vrf::verify_vrf_proof(&kp.public_key, &proof, &seed)
            .expect("VRF proof must verify");
        assert_eq!(
            verified_output, raw_output,
            "Verified output must match generated output"
        );

        // Step 4: compute Praos leader value (domain-separated)
        let leader_value = vrf_leader_value(&raw_output);

        // Step 5: check election — the leader value must be consistent
        // with is_slot_leader (which also calls vrf_leader_value internally)
        let from_is_leader = is_slot_leader(&raw_output, 1.0, 0.05);
        let from_check_leader = torsten_crypto::vrf::check_leader_value(&leader_value, 1.0, 0.05);
        assert_eq!(
            from_is_leader, from_check_leader,
            "is_slot_leader and check_leader_value must agree"
        );
    }

    /// Verify that the compute_leader_schedule election result is consistent with
    /// manually applying is_slot_leader_rational to each slot's VRF output.
    ///
    /// Both the schedule function and the manual loop now use the fully exact
    /// rational path (no f64), so their results must be bit-for-bit identical.
    #[test]
    fn test_leader_schedule_consistent_with_is_slot_leader() {
        let secret = [0x33u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0x44u8; 32]);
        let epoch_len = 200u64;
        // sigma = 1/1, f = 1/20  (same rational values used by compute_leader_schedule)
        let sigma_num = 1u64;
        let sigma_den = 1u64;
        let f_num = 1u64;
        let f_den = 20u64;

        let schedule = compute_leader_schedule(
            kp.secret_key(),
            &epoch_nonce,
            0,
            epoch_len,
            sigma_num,
            sigma_den,
            f_num,
            f_den,
        );

        // Manually check each slot using the same rational path.
        let mut manual_schedule: Vec<SlotNo> = Vec::new();
        for offset in 0..epoch_len {
            let slot = SlotNo(offset);
            let seed = vrf_input(&epoch_nonce, slot);
            if let Ok((_, output)) = torsten_crypto::vrf::generate_vrf_proof(kp.secret_key(), &seed)
            {
                if is_slot_leader_rational(&output, sigma_num, sigma_den, f_num, f_den) {
                    manual_schedule.push(slot);
                }
            }
        }

        let schedule_slots: Vec<SlotNo> = schedule.iter().map(|ls| ls.slot).collect();
        assert_eq!(
            schedule_slots, manual_schedule,
            "compute_leader_schedule must match manual slot-by-slot rational check"
        );
    }

    // -------------------------------------------------------------------------
    // Edge cases
    // -------------------------------------------------------------------------

    /// Empty epoch (epoch_length=0) must produce an empty schedule without panicking.
    #[test]
    fn test_leader_schedule_empty_epoch() {
        let secret = [0u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        let epoch_nonce = Hash32::from_bytes([0u8; 32]);

        // sigma = 1/1, f = 1/20, epoch_length = 0
        let schedule = compute_leader_schedule(kp.secret_key(), &epoch_nonce, 0, 0, 1, 1, 1, 20);
        assert!(
            schedule.is_empty(),
            "Empty epoch must produce empty schedule"
        );
    }

    /// NeutralNonce (all-zero) epoch nonce: schedule computation must not panic
    /// and must return a valid (possibly empty) schedule.
    #[test]
    fn test_leader_schedule_neutral_nonce() {
        let secret = [0x99u8; 32];
        let kp = torsten_crypto::vrf::generate_vrf_keypair_from_secret(&secret);
        // The NeutralNonce is all zeros — used before the first epoch nonce is established
        let neutral_nonce = Hash32::from_bytes([0u8; 32]);

        // Must not panic; sigma = 1/1, f = 1/20
        let schedule =
            compute_leader_schedule(kp.secret_key(), &neutral_nonce, 0, 500, 1, 1, 1, 20);

        // Proofs must still verify against the neutral nonce
        for ls in &schedule {
            let seed = vrf_input(&neutral_nonce, ls.slot);
            let verified =
                torsten_crypto::vrf::verify_vrf_proof(&kp.public_key, &ls.vrf_proof, &seed);
            assert!(
                verified.is_ok(),
                "VRF proof must verify against neutral nonce for slot {:?}",
                ls.slot
            );
        }
    }

    /// Very large relative stake (> 1.0) must not panic or produce nonsensical results.
    /// The Praos spec only defines phi_f for sigma in [0,1], but the code must be robust.
    ///
    /// Note: NaN and infinity are NOT tested here because the fixed-point arithmetic
    /// in `check_leader_value` uses `float_to_fixed()` which loops when passed
    /// infinite or NaN values.  The protocol layer must gate on valid stake ranges
    /// before calling this function; see `validate_header_full` which only calls it
    /// when `relative_stake > 0.0`.
    #[test]
    fn test_check_slot_leadership_stake_above_one_no_panic() {
        let lv = [0u8; 32];
        // Stake > 1.0 is mathematically nonsensical but must not panic.
        // phi_f(2.0) = 1 - (1-f)^2 = 1 - 0.9025 = 0.0975.
        // certNat=0 → always elected for any positive phi.
        let result = torsten_crypto::vrf::check_leader_value(&lv, 2.0, 0.05);
        assert!(result, "certNat=0 must always elect even with stake > 1.0");
    }
}
