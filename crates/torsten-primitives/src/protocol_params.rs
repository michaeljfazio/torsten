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

    // Conway governance
    pub drep_deposit: Lovelace,
    pub drep_activity: u64,
    pub gov_action_deposit: Lovelace,
    pub gov_action_lifetime: u64,
    pub committee_min_size: u64,
    pub committee_max_term_length: u64,

    // Governance voting thresholds (as rationals)
    /// DRep voting threshold for ParameterChange
    pub dvt_p_param_change: Rational,
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
    /// SPO voting threshold for HardForkInitiation
    pub pvt_hard_fork: Rational,
    /// SPO voting threshold for NoConfidence/UpdateCommittee
    pub pvt_committee: Rational,

    // Protocol version
    pub protocol_version_major: u64,
    pub protocol_version_minor: u64,

    // Consensus
    /// Active slot coefficient (probability of a slot having a block)
    #[serde(default = "default_active_slot_coeff")]
    pub active_slots_coeff: f64,
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

    /// Calculate minimum UTxO value (ada-only)
    pub fn min_utxo_value(&self) -> Lovelace {
        // Minimum UTxO for a simple ada-only output (roughly 29 bytes overhead)
        Lovelace(self.ada_per_utxo_byte.0 * 29)
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
            drep_deposit: Lovelace(500_000_000),
            drep_activity: 20,
            gov_action_deposit: Lovelace(100_000_000_000),
            gov_action_lifetime: 6,
            committee_min_size: 7,
            committee_max_term_length: 146,
            dvt_p_param_change: Rational {
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
            pvt_hard_fork: Rational {
                numerator: 51,
                denominator: 100,
            },
            pvt_committee: Rational {
                numerator: 51,
                denominator: 100,
            },
            protocol_version_major: 9,
            protocol_version_minor: 0,
            active_slots_coeff: 0.05,
        }
    }
}
