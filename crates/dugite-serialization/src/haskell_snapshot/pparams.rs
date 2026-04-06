//! Conway PParams array(31) decoder for Haskell ledger snapshots.
//!
//! Haskell serialises `ConwayPParams StrictMaybe ConwayEra` as a flat CBOR
//! array of exactly 31 elements in a fixed positional order.  This module
//! decodes that encoding into dugite's [`ProtocolParameters`] struct.
//!
//! # Field order (verified against preview epoch 1259)
//!
//! ```text
//!  [0]  txFeePerByte          (minFeeA)                uint
//!  [1]  txFeeFixed            (minFeeB)                uint
//!  [2]  maxBlockBodySize                               uint
//!  [3]  maxTxSize                                      uint
//!  [4]  maxBlockHeaderSize                             uint
//!  [5]  keyDeposit                                     uint (lovelace)
//!  [6]  poolDeposit                                    uint (lovelace)
//!  [7]  eMax                                           uint
//!  [8]  nOpt                                           uint
//!  [9]  a0                    (pledge influence)       rational
//! [10]  rho                   (monetary expansion)     rational
//! [11]  tau                   (treasury growth)        rational
//! [12]  protocolVersion                                array(2) [major, minor]
//! [13]  minPoolCost                                    uint (lovelace)
//! [14]  coinsPerUTxOByte      (adaPerUTxOByte)         uint (lovelace)
//! [15]  costModels                                     map {0: [...], 1: [...], 2: [...]}
//! [16]  prices                (exUnitPrices)           array(2) of rationals
//! [17]  maxTxExUnits                                   array(2) [mem, steps]
//! [18]  maxBlockExUnits                                array(2) [mem, steps]
//! [19]  maxValSize                                     uint
//! [20]  collateralPercentage                           uint
//! [21]  maxCollateralInputs                            uint
//! [22]  poolVotingThresholds                           array(5) of rationals
//! [23]  dRepVotingThresholds                           array(10) of rationals
//! [24]  committeeMinSize                               uint
//! [25]  committeeMaxTermLength                         uint (epochs)
//! [26]  govActionLifetime                              uint (epochs)
//! [27]  govActionDeposit                               uint (lovelace)
//! [28]  dRepDeposit                                    uint (lovelace)
//! [29]  dRepActivity                                   uint (epochs)
//! [30]  minFeeRefScriptCostPerByte                     rational (e.g. 15/1)
//! ```
//!
//! ## Cost model key → Plutus version mapping
//! - `0` → PlutusV1
//! - `1` → PlutusV2
//! - `2` → PlutusV3
//!
//! ## Pool voting threshold order (index 22)
//! `[pvtMotionNoConfidence, pvtCommitteeNormal, pvtCommitteeNoConfidence,
//!   pvtHardForkInitiation, pvtPPSecurityGroup]`
//!
//! ## DRep voting threshold order (index 23)
//! `[dvtMotionNoConfidence, dvtCommitteeNormal, dvtCommitteeNoConfidence,
//!   dvtUpdateToConstitution, dvtHardForkInitiation,
//!   dvtPPNetworkGroup, dvtPPEconomicGroup, dvtPPTechnicalGroup,
//!   dvtPPGovGroup, dvtTreasuryWithdrawal]`

use crate::error::SerializationError;
use dugite_primitives::{
    protocol_params::ProtocolParameters,
    transaction::{CostModels, ExUnitPrices, ExUnits, Rational},
    value::Lovelace,
};

use super::cbor_utils::{
    decode_array_len, decode_array_len_or_indef, decode_int, decode_map_len, decode_rational,
    decode_uint,
};

// ── Public entry point ──────────────────────────────────────────────────────

/// Decode Conway `PParams` from a CBOR `array(31)`.
///
/// Returns `(ProtocolParameters, bytes_consumed)`.
///
/// The two fields that are **not** carried in PParams itself —
/// `active_slots_coeff` (genesis) and `d` (deprecated) — are initialised
/// to safe defaults: `0.05` and `0/1` respectively.  Callers that have
/// access to genesis configuration should overwrite `active_slots_coeff`
/// after calling this function.
pub fn decode_pparams(data: &[u8]) -> Result<(ProtocolParameters, usize), SerializationError> {
    let mut off = 0;

    // ── outer array header ──────────────────────────────────────────────────
    let (arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if arr_len != 31 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: expected array(31), got array({arr_len})"
        )));
    }

    // ── [0] txFeePerByte (minFeeA) ──────────────────────────────────────────
    let (min_fee_a, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [1] txFeeFixed (minFeeB) ────────────────────────────────────────────
    let (min_fee_b, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [2] maxBlockBodySize ────────────────────────────────────────────────
    let (max_block_body_size, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [3] maxTxSize ───────────────────────────────────────────────────────
    let (max_tx_size, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [4] maxBlockHeaderSize ──────────────────────────────────────────────
    let (max_block_header_size, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [5] keyDeposit ──────────────────────────────────────────────────────
    let (key_deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [6] poolDeposit ─────────────────────────────────────────────────────
    let (pool_deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [7] eMax ────────────────────────────────────────────────────────────
    let (e_max, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [8] nOpt ────────────────────────────────────────────────────────────
    let (n_opt, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [9] a0 (pool pledge influence, rational) ────────────────────────────
    let ((a0_num, a0_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [10] rho (monetary expansion, rational) ─────────────────────────────
    let ((rho_num, rho_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [11] tau (treasury growth rate, rational) ───────────────────────────
    let ((tau_num, tau_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [12] protocolVersion: array(2) [major, minor] ──────────────────────
    let (pv_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if pv_arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: protocolVersion must be array(2), got array({pv_arr_len})"
        )));
    }
    let (protocol_version_major, n) = decode_uint(&data[off..])?;
    off += n;
    let (protocol_version_minor, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [13] minPoolCost ────────────────────────────────────────────────────
    let (min_pool_cost, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [14] coinsPerUTxOByte ───────────────────────────────────────────────
    let (ada_per_utxo_byte, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [15] costModels ─────────────────────────────────────────────────────
    let (cost_models, n) = decode_cost_models(&data[off..])?;
    off += n;

    // ── [16] prices: array(2) [mem_rational, step_rational] ─────────────────
    let (prices_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if prices_arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: prices must be array(2), got array({prices_arr_len})"
        )));
    }
    let ((mem_num, mem_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((step_num, step_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [17] maxTxExUnits: array(2) [mem, steps] ────────────────────────────
    let (tx_ex_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if tx_ex_arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: maxTxExUnits must be array(2), got array({tx_ex_arr_len})"
        )));
    }
    let (max_tx_ex_mem, n) = decode_uint(&data[off..])?;
    off += n;
    let (max_tx_ex_steps, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [18] maxBlockExUnits: array(2) [mem, steps] ─────────────────────────
    let (blk_ex_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if blk_ex_arr_len != 2 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: maxBlockExUnits must be array(2), got array({blk_ex_arr_len})"
        )));
    }
    let (max_block_ex_mem, n) = decode_uint(&data[off..])?;
    off += n;
    let (max_block_ex_steps, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [19] maxValSize ─────────────────────────────────────────────────────
    let (max_val_size, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [20] collateralPercentage ───────────────────────────────────────────
    let (collateral_percentage, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [21] maxCollateralInputs ────────────────────────────────────────────
    let (max_collateral_inputs, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [22] poolVotingThresholds: array(5) of rationals ────────────────────
    // Order: [pvtMotionNoConfidence, pvtCommitteeNormal, pvtCommitteeNoConfidence,
    //         pvtHardForkInitiation, pvtPPSecurityGroup]
    let (pvt_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if pvt_arr_len != 5 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: poolVotingThresholds must be array(5), got array({pvt_arr_len})"
        )));
    }
    let ((pvt_no_conf_num, pvt_no_conf_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((pvt_comm_norm_num, pvt_comm_norm_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((pvt_comm_no_conf_num, pvt_comm_no_conf_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((pvt_hf_num, pvt_hf_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((pvt_pp_sec_num, pvt_pp_sec_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [23] dRepVotingThresholds: array(10) of rationals ───────────────────
    // Order: [dvtMotionNoConfidence, dvtCommitteeNormal, dvtCommitteeNoConfidence,
    //         dvtUpdateToConstitution, dvtHardForkInitiation,
    //         dvtPPNetworkGroup, dvtPPEconomicGroup, dvtPPTechnicalGroup,
    //         dvtPPGovGroup, dvtTreasuryWithdrawal]
    let (dvt_arr_len, n) = decode_array_len(&data[off..])?;
    off += n;
    if dvt_arr_len != 10 {
        return Err(SerializationError::CborDecode(format!(
            "PParams: dRepVotingThresholds must be array(10), got array({dvt_arr_len})"
        )));
    }
    let ((dvt_no_conf_num, dvt_no_conf_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_comm_norm_num, dvt_comm_norm_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_comm_no_conf_num, dvt_comm_no_conf_den), n) = decode_rational(&data[off..])?;
    off += n;
    // dvtUpdateToConstitution maps to dvt_constitution in our struct
    let ((dvt_constitution_num, dvt_constitution_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_hf_num, dvt_hf_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_pp_net_num, dvt_pp_net_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_pp_eco_num, dvt_pp_eco_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_pp_tech_num, dvt_pp_tech_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_pp_gov_num, dvt_pp_gov_den), n) = decode_rational(&data[off..])?;
    off += n;
    let ((dvt_treasury_num, dvt_treasury_den), n) = decode_rational(&data[off..])?;
    off += n;

    // ── [24] committeeMinSize ───────────────────────────────────────────────
    let (committee_min_size, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [25] committeeMaxTermLength (epochs) ────────────────────────────────
    let (committee_max_term_length, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [26] govActionLifetime (epochs) ─────────────────────────────────────
    let (gov_action_lifetime, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [27] govActionDeposit ───────────────────────────────────────────────
    let (gov_action_deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [28] dRepDeposit ────────────────────────────────────────────────────
    let (drep_deposit, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [29] dRepActivity (epochs) ──────────────────────────────────────────
    let (drep_activity, n) = decode_uint(&data[off..])?;
    off += n;

    // ── [30] minFeeRefScriptCostPerByte (rational, e.g. 15/1) ───────────────
    let (min_fee_ref_script_cost_per_byte, n) = decode_min_fee_ref_script(&data[off..])?;
    off += n;

    // ── Assemble ────────────────────────────────────────────────────────────
    let pp = ProtocolParameters {
        min_fee_a,
        min_fee_b,
        max_block_body_size,
        max_tx_size,
        max_block_header_size,
        key_deposit: Lovelace(key_deposit),
        pool_deposit: Lovelace(pool_deposit),
        e_max,
        n_opt,
        a0: Rational {
            numerator: a0_num,
            denominator: a0_den,
        },
        rho: Rational {
            numerator: rho_num,
            denominator: rho_den,
        },
        tau: Rational {
            numerator: tau_num,
            denominator: tau_den,
        },
        protocol_version_major,
        protocol_version_minor,
        min_pool_cost: Lovelace(min_pool_cost),
        ada_per_utxo_byte: Lovelace(ada_per_utxo_byte),
        cost_models,
        execution_costs: ExUnitPrices {
            mem_price: Rational {
                numerator: mem_num,
                denominator: mem_den,
            },
            step_price: Rational {
                numerator: step_num,
                denominator: step_den,
            },
        },
        max_tx_ex_units: ExUnits {
            mem: max_tx_ex_mem,
            steps: max_tx_ex_steps,
        },
        max_block_ex_units: ExUnits {
            mem: max_block_ex_mem,
            steps: max_block_ex_steps,
        },
        max_val_size,
        collateral_percentage,
        max_collateral_inputs,
        min_fee_ref_script_cost_per_byte,

        // Conway governance
        committee_min_size,
        committee_max_term_length,
        gov_action_lifetime,
        gov_action_deposit: Lovelace(gov_action_deposit),
        drep_deposit: Lovelace(drep_deposit),
        drep_activity,

        // Pool voting thresholds — order from Haskell PoolVotingThresholds:
        // pvtMotionNoConfidence, pvtCommitteeNormal, pvtCommitteeNoConfidence,
        // pvtHardForkInitiation, pvtPPSecurityGroup
        pvt_motion_no_confidence: Rational {
            numerator: pvt_no_conf_num,
            denominator: pvt_no_conf_den,
        },
        pvt_committee_normal: Rational {
            numerator: pvt_comm_norm_num,
            denominator: pvt_comm_norm_den,
        },
        pvt_committee_no_confidence: Rational {
            numerator: pvt_comm_no_conf_num,
            denominator: pvt_comm_no_conf_den,
        },
        pvt_hard_fork: Rational {
            numerator: pvt_hf_num,
            denominator: pvt_hf_den,
        },
        pvt_pp_security_group: Rational {
            numerator: pvt_pp_sec_num,
            denominator: pvt_pp_sec_den,
        },

        // DRep voting thresholds — order from Haskell DRepVotingThresholds:
        // dvtMotionNoConfidence, dvtCommitteeNormal, dvtCommitteeNoConfidence,
        // dvtUpdateToConstitution, dvtHardForkInitiation,
        // dvtPPNetworkGroup, dvtPPEconomicGroup, dvtPPTechnicalGroup,
        // dvtPPGovGroup, dvtTreasuryWithdrawal
        dvt_no_confidence: Rational {
            numerator: dvt_no_conf_num,
            denominator: dvt_no_conf_den,
        },
        dvt_committee_normal: Rational {
            numerator: dvt_comm_norm_num,
            denominator: dvt_comm_norm_den,
        },
        dvt_committee_no_confidence: Rational {
            numerator: dvt_comm_no_conf_num,
            denominator: dvt_comm_no_conf_den,
        },
        // dvtUpdateToConstitution → dvt_constitution in our struct
        dvt_constitution: Rational {
            numerator: dvt_constitution_num,
            denominator: dvt_constitution_den,
        },
        dvt_hard_fork: Rational {
            numerator: dvt_hf_num,
            denominator: dvt_hf_den,
        },
        dvt_pp_network_group: Rational {
            numerator: dvt_pp_net_num,
            denominator: dvt_pp_net_den,
        },
        dvt_pp_economic_group: Rational {
            numerator: dvt_pp_eco_num,
            denominator: dvt_pp_eco_den,
        },
        dvt_pp_technical_group: Rational {
            numerator: dvt_pp_tech_num,
            denominator: dvt_pp_tech_den,
        },
        dvt_pp_gov_group: Rational {
            numerator: dvt_pp_gov_num,
            denominator: dvt_pp_gov_den,
        },
        dvt_treasury_withdrawal: Rational {
            numerator: dvt_treasury_num,
            denominator: dvt_treasury_den,
        },

        // Fields not carried in PParams; callers should overwrite active_slots_coeff
        // from genesis after constructing the parameters.
        active_slots_coeff: 0.05,
        d: Rational {
            numerator: 0,
            denominator: 1,
        },
    };

    Ok((pp, off))
}

// ── Cost model decoder ──────────────────────────────────────────────────────

/// Decode the `costModels` field: a CBOR map from language-version key to a
/// flat array of signed integer operation costs.
///
/// Key mapping:
/// - `0` → PlutusV1
/// - `1` → PlutusV2
/// - `2` → PlutusV3
///
/// Both definite-length (`map(n)`) and indefinite-length (`*_`) maps are
/// accepted, since the Haskell serialiser has historically used both.
/// Unknown keys are skipped so that future Plutus versions do not cause
/// decode failures.
///
/// Cost-model entries are signed integers (operations can carry negative
/// default costs in some experimental builds).
pub fn decode_cost_models(data: &[u8]) -> Result<(CostModels, usize), SerializationError> {
    let mut off = 0;

    let (maybe_len, n) = decode_map_len(&data[off..])?;
    off += n;

    let mut plutus_v1: Option<Vec<i64>> = None;
    let mut plutus_v2: Option<Vec<i64>> = None;
    let mut plutus_v3: Option<Vec<i64>> = None;

    // Decode each key→value pair; stop when we've consumed `maybe_len` entries
    // (definite) or hit the break byte 0xff (indefinite).
    let mut entries_decoded = 0usize;
    loop {
        // Termination check
        match maybe_len {
            Some(map_len) => {
                if entries_decoded >= map_len {
                    break;
                }
            }
            None => {
                // Indefinite-length map: 0xff is the break byte
                if data.get(off) == Some(&0xff) {
                    off += 1; // consume the break byte
                    break;
                }
            }
        }

        // Key: uint language version
        let (key, n) = decode_uint(&data[off..])?;
        off += n;

        // Value: array of signed integers (cost values)
        let costs = decode_cost_array(&data[off..])?;
        let n = costs.1;
        let costs = costs.0;
        off += n;

        match key {
            0 => plutus_v1 = Some(costs),
            1 => plutus_v2 = Some(costs),
            2 => plutus_v3 = Some(costs),
            // Silently ignore unknown language versions so future Plutus
            // versions don't break deserialization.
            _ => {}
        }

        entries_decoded += 1;
    }

    Ok((
        CostModels {
            plutus_v1,
            plutus_v2,
            plutus_v3,
        },
        off,
    ))
}

/// Decode an array of signed cost-model integers, returning `(values, bytes_consumed)`.
///
/// Each entry is decoded as a signed integer (`decode_int`) to correctly
/// handle negative cost values that can appear in some experimental cost
/// model configurations.
///
/// Supports both definite-length and indefinite-length CBOR arrays, since the
/// Haskell node may use either encoding depending on the serialisation path.
fn decode_cost_array(data: &[u8]) -> Result<(Vec<i64>, usize), SerializationError> {
    let mut off = 0;

    let (maybe_len, n) = decode_array_len_or_indef(&data[off..])?;
    off += n;

    match maybe_len {
        Some(arr_len) => {
            let mut costs = Vec::with_capacity(arr_len);
            for _ in 0..arr_len {
                let (v, n) = decode_int(&data[off..])?;
                off += n;
                costs.push(v);
            }
            Ok((costs, off))
        }
        None => {
            // Indefinite-length array: read until break byte 0xff.
            let mut costs = Vec::new();
            while off < data.len() && data[off] != 0xff {
                let (v, n) = decode_int(&data[off..])?;
                off += n;
                costs.push(v);
            }
            if off >= data.len() {
                return Err(SerializationError::CborDecode(
                    "cost array: missing break byte in indefinite array".into(),
                ));
            }
            off += 1; // consume 0xff break byte
            Ok((costs, off))
        }
    }
}

// ── minFeeRefScriptCostPerByte decoder ─────────────────────────────────────

/// Decode the `minFeeRefScriptCostPerByte` field.
///
/// Haskell encodes this as a rational (e.g. `tag(30) [15, 1]` for the value
/// 15).  Some older snapshots or testing environments may encode it as a plain
/// unsigned integer.  Both forms are accepted; the integer value of the
/// numerator is returned (i.e. `floor(num / den)` with an error if den == 0).
///
/// For the expected case of `15/1`, this returns `15`.
pub fn decode_min_fee_ref_script(data: &[u8]) -> Result<(u64, usize), SerializationError> {
    // Peek at the first byte(s) to decide encoding:
    // - 0xd8 0x1e → tag(30) followed by array(2) — rational
    // - 0x82       → bare array(2) — rational without tag
    // - any major-0 byte → plain uint
    if data.is_empty() {
        return Err(SerializationError::CborDecode(
            "minFeeRefScriptCostPerByte: unexpected end of input".into(),
        ));
    }

    let is_rational = (data[0] == 0xd8 && data.get(1) == Some(&0x1e)) || ((data[0] >> 5) == 4); // major 4 = array (bare rational)

    if is_rational {
        let ((num, den), n) = decode_rational(data)?;
        if den == 0 {
            return Err(SerializationError::CborDecode(
                "minFeeRefScriptCostPerByte: rational denominator is zero".into(),
            ));
        }
        // Truncating division is correct: Haskell always encodes 15/1 or
        // similar whole-number rationals for this parameter.
        Ok((num / den, n))
    } else {
        // Plain uint fallback
        let (v, n) = decode_uint(data)?;
        Ok((v, n))
    }
}
