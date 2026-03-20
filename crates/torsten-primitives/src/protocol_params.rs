use crate::transaction::{CostModels, ExUnitPrices, ExUnits, Rational};
use crate::value::Lovelace;
use serde::{Deserialize, Serialize};

/// Complete protocol parameters (Conway era)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolParameters {
    // Fee calculation: fee = min_fee_a * tx_size + min_fee_b
    pub min_fee_a: u64,
    pub min_fee_b: u64,

    // Size limits
    pub max_block_body_size: u64,
    pub max_tx_size: u64,
    pub max_block_header_size: u64,

    // Staking
    pub key_deposit: Lovelace,
    pub pool_deposit: Lovelace,
    pub e_max: u64,    // maximum epoch for pool retirement
    pub n_opt: u64,    // desired number of stake pools
    pub a0: Rational,  // pool pledge influence
    pub rho: Rational, // monetary expansion
    pub tau: Rational, // treasury growth rate

    // Minimum values
    pub min_pool_cost: Lovelace,
    pub ada_per_utxo_byte: Lovelace,

    // Plutus
    pub cost_models: CostModels,
    pub execution_costs: ExUnitPrices,
    pub max_tx_ex_units: ExUnits,
    pub max_block_ex_units: ExUnits,
    pub max_val_size: u64,
    pub collateral_percentage: u64,
    pub max_collateral_inputs: u64,
    pub min_fee_ref_script_cost_per_byte: u64,

    // Conway governance
    pub drep_deposit: Lovelace,
    pub drep_activity: u64,
    pub gov_action_deposit: Lovelace,
    pub gov_action_lifetime: u64,
    pub committee_min_size: u64,
    pub committee_max_term_length: u64,

    // Governance voting thresholds (as rationals)
    /// DRep voting threshold for PP changes — network group
    pub dvt_pp_network_group: Rational,
    /// DRep voting threshold for PP changes — economic group
    pub dvt_pp_economic_group: Rational,
    /// DRep voting threshold for PP changes — technical group
    pub dvt_pp_technical_group: Rational,
    /// DRep voting threshold for PP changes — governance group
    pub dvt_pp_gov_group: Rational,
    /// DRep voting threshold for HardForkInitiation
    pub dvt_hard_fork: Rational,
    /// DRep voting threshold for NoConfidence
    pub dvt_no_confidence: Rational,
    /// DRep voting threshold for UpdateCommittee (normal state)
    pub dvt_committee_normal: Rational,
    /// DRep voting threshold for UpdateCommittee (no confidence state)
    pub dvt_committee_no_confidence: Rational,
    /// DRep voting threshold for NewConstitution
    pub dvt_constitution: Rational,
    /// DRep voting threshold for TreasuryWithdrawals
    pub dvt_treasury_withdrawal: Rational,
    /// SPO voting threshold for MotionNoConfidence
    pub pvt_motion_no_confidence: Rational,
    /// SPO voting threshold for UpdateCommittee (normal state)
    pub pvt_committee_normal: Rational,
    /// SPO voting threshold for UpdateCommittee (no confidence state)
    pub pvt_committee_no_confidence: Rational,
    /// SPO voting threshold for HardForkInitiation
    pub pvt_hard_fork: Rational,
    /// SPO voting threshold for security-relevant protocol parameter changes
    pub pvt_pp_security_group: Rational,

    // Protocol version
    pub protocol_version_major: u64,
    pub protocol_version_minor: u64,

    // Consensus
    /// Active slot coefficient (probability of a slot having a block)
    #[serde(default = "default_active_slot_coeff")]
    pub active_slots_coeff: f64,

    /// Decentralisation parameter (d): fraction of slots reserved for BFT
    /// overlay schedule. Range [0, 1] where 0 = fully decentralised (Praos),
    /// 1 = fully federated (BFT). Deprecated since Babbage (always 0).
    ///
    /// Stored as rational numerator/denominator. When d >= 0.8, the Haskell
    /// reward calculation forces eta = 1 (no performance adjustment).
    #[serde(default)]
    pub d: Rational,
}

fn default_active_slot_coeff() -> f64 {
    0.05
}

impl ProtocolParameters {
    /// Calculate the minimum fee for a transaction
    pub fn min_fee(&self, tx_size: u64) -> Lovelace {
        Lovelace(self.min_fee_a * tx_size + self.min_fee_b)
    }

    /// Active slot coefficient (1/20 = 0.05 on mainnet)
    pub fn active_slot_coeff(&self) -> f64 {
        self.active_slots_coeff
    }

    /// Active slot coefficient as a rational (numerator, denominator).
    ///
    /// Converts the f64 `active_slots_coeff` to an exact rational by searching
    /// for the smallest denominator that reconstructs the float exactly.
    /// For mainnet (0.05) this returns (1, 20).
    pub fn active_slot_coeff_rational(&self) -> (u64, u64) {
        f64_to_rational(self.active_slots_coeff)
    }

    /// Calculate minimum UTxO value (ada-only)
    /// Minimum UTxO for a simple ADA-only output (no multi-assets, no datum)
    pub fn min_utxo_value(&self) -> Lovelace {
        // Babbage formula: coins_per_utxo_byte * (160 + output_size)
        // Simple ADA-only output is ~29 bytes serialized
        Lovelace(self.ada_per_utxo_byte.0 * (160 + 29))
    }

    /// Minimum UTxO for an output with a specific serialized size.
    /// Uses the Babbage/Conway formula: coins_per_utxo_byte * (160 + output_size)
    /// where 160 is the constant overhead for the UTxO entry itself.
    pub fn min_utxo_for_output_size(&self, output_size_bytes: u64) -> Lovelace {
        Lovelace(self.ada_per_utxo_byte.0 * (160 + output_size_bytes))
    }

    /// Default mainnet parameters (Conway era, approximate)
    pub fn mainnet_defaults() -> Self {
        ProtocolParameters {
            min_fee_a: 44,
            min_fee_b: 155381,
            max_block_body_size: 90112,
            max_tx_size: 16384,
            max_block_header_size: 1100,
            key_deposit: Lovelace(2_000_000),
            pool_deposit: Lovelace(500_000_000),
            e_max: 18,
            n_opt: 500,
            a0: Rational {
                numerator: 3,
                denominator: 10,
            },
            rho: Rational {
                numerator: 3,
                denominator: 1000,
            },
            tau: Rational {
                numerator: 2,
                denominator: 10,
            },
            min_pool_cost: Lovelace(170_000_000),
            ada_per_utxo_byte: Lovelace(4310),
            cost_models: CostModels {
                plutus_v1: None,
                plutus_v2: None,
                plutus_v3: None,
            },
            execution_costs: ExUnitPrices {
                mem_price: Rational {
                    numerator: 577,
                    denominator: 10000,
                },
                step_price: Rational {
                    numerator: 721,
                    denominator: 10000000,
                },
            },
            max_tx_ex_units: ExUnits {
                mem: 14_000_000,
                steps: 10_000_000_000,
            },
            max_block_ex_units: ExUnits {
                mem: 62_000_000,
                steps: 40_000_000_000,
            },
            max_val_size: 5000,
            collateral_percentage: 150,
            max_collateral_inputs: 3,
            min_fee_ref_script_cost_per_byte: 15,
            drep_deposit: Lovelace(500_000_000),
            drep_activity: 20,
            gov_action_deposit: Lovelace(100_000_000_000),
            gov_action_lifetime: 6,
            committee_min_size: 7,
            committee_max_term_length: 146,
            dvt_pp_network_group: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_pp_economic_group: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_pp_technical_group: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_pp_gov_group: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_hard_fork: Rational {
                numerator: 60,
                denominator: 100,
            },
            dvt_no_confidence: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_committee_normal: Rational {
                numerator: 67,
                denominator: 100,
            },
            dvt_committee_no_confidence: Rational {
                numerator: 60,
                denominator: 100,
            },
            dvt_constitution: Rational {
                numerator: 75,
                denominator: 100,
            },
            dvt_treasury_withdrawal: Rational {
                numerator: 67,
                denominator: 100,
            },
            pvt_motion_no_confidence: Rational {
                numerator: 51,
                denominator: 100,
            },
            pvt_committee_normal: Rational {
                numerator: 51,
                denominator: 100,
            },
            pvt_committee_no_confidence: Rational {
                numerator: 51,
                denominator: 100,
            },
            pvt_hard_fork: Rational {
                numerator: 51,
                denominator: 100,
            },
            pvt_pp_security_group: Rational {
                numerator: 51,
                denominator: 100,
            },
            protocol_version_major: 9,
            protocol_version_minor: 0,
            active_slots_coeff: 0.05,
            d: Rational {
                numerator: 0,
                denominator: 1,
            },
        }
    }
}

/// Convert an f64 to a rational numerator/denominator pair.
///
/// Searches for the smallest denominator (up to common Cardano denominators,
/// then up to 1,000,000) that reconstructs the float exactly. For Cardano
/// protocol parameters this always finds a clean fraction (e.g., 0.05 = 1/20).
pub fn f64_to_rational(value: f64) -> (u64, u64) {
    if value == 0.0 {
        return (0, 1);
    }
    if value == 1.0 {
        return (1, 1);
    }
    // Try common denominators first (covers all known Cardano active_slot_coeff values)
    for den in [1, 2, 4, 5, 10, 20, 25, 50, 100, 200, 1000, 10000] {
        let num = (value * den as f64).round() as u64;
        let reconstructed = num as f64 / den as f64;
        if (reconstructed - value).abs() < 1e-15 {
            let g = gcd(num, den);
            return (num / g, den / g);
        }
    }
    // Fallback: use 1_000_000 as denominator
    let den = 1_000_000u64;
    let num = (value * den as f64).round() as u64;
    let g = gcd(num, den);
    (num / g, den / g)
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

/// Compute `ceil(numerator * k / f)` using integer arithmetic, where `f = f_num / f_den`.
///
/// This computes `ceil(numerator * k * f_den / f_num)` without floating-point.
/// Used for stability window (3k/f) and randomness stabilisation window (4k/f).
pub fn ceiling_div_by_rational(multiplier: u64, k: u64, f_num: u64, f_den: u64) -> u64 {
    assert!(f_num > 0, "active_slot_coeff numerator must be > 0");
    let numerator = multiplier as u128 * k as u128 * f_den as u128;
    let denominator = f_num as u128;
    numerator.div_ceil(denominator) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_f64_to_rational_mainnet() {
        // Mainnet: active_slot_coeff = 0.05 = 1/20
        assert_eq!(f64_to_rational(0.05), (1, 20));
    }

    #[test]
    fn test_f64_to_rational_common_values() {
        assert_eq!(f64_to_rational(0.0), (0, 1));
        assert_eq!(f64_to_rational(1.0), (1, 1));
        assert_eq!(f64_to_rational(0.5), (1, 2));
        assert_eq!(f64_to_rational(0.1), (1, 10));
        assert_eq!(f64_to_rational(0.25), (1, 4));
    }

    #[test]
    fn test_active_slot_coeff_rational() {
        let params = ProtocolParameters::mainnet_defaults();
        assert_eq!(params.active_slot_coeff_rational(), (1, 20));
    }

    #[test]
    fn test_ceiling_div_by_rational_mainnet_randomness_window() {
        // Mainnet: 4 * 2160 / 0.05 = 4 * 2160 * 20 / 1 = 172800
        assert_eq!(ceiling_div_by_rational(4, 2160, 1, 20), 172800);
    }

    #[test]
    fn test_ceiling_div_by_rational_mainnet_stability_window() {
        // Mainnet: 3 * 2160 / 0.05 = 3 * 2160 * 20 / 1 = 129600
        assert_eq!(ceiling_div_by_rational(3, 2160, 1, 20), 129600);
    }

    #[test]
    fn test_ceiling_div_by_rational_preview() {
        // Preview: k=432, f=0.05 → 4 * 432 / 0.05 = 34560
        assert_eq!(ceiling_div_by_rational(4, 432, 1, 20), 34560);
        // 3k/f = 3 * 432 / 0.05 = 25920
        assert_eq!(ceiling_div_by_rational(3, 432, 1, 20), 25920);
    }

    #[test]
    fn test_ceiling_div_rounds_up() {
        // 4 * 1 / (1/3) = 12 exactly
        assert_eq!(ceiling_div_by_rational(4, 1, 1, 3), 12);
        // 4 * 1 / (2/3) = 6 exactly
        assert_eq!(ceiling_div_by_rational(4, 1, 2, 3), 6);
        // 4 * 1 / (1/7) = 28 exactly
        assert_eq!(ceiling_div_by_rational(4, 1, 1, 7), 28);
        // 3 * 1 / (2/5) = 7.5 → ceil = 8
        assert_eq!(ceiling_div_by_rational(3, 1, 2, 5), 8);
    }

    #[test]
    fn test_ceiling_div_exact_division() {
        // When the result is exact, no rounding should occur
        // 4 * 100 * 20 / 1 = 8000
        assert_eq!(ceiling_div_by_rational(4, 100, 1, 20), 8000);
    }
}
