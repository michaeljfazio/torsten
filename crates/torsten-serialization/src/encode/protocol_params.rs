use crate::cbor::*;
use torsten_primitives::transaction::*;

use super::certificate::encode_rational;

/// Encode a ProtocolParamUpdate as a CBOR map with integer keys per Conway CDDL.
///
/// Only fields that are `Some` are included in the map (sparse encoding).
/// Key mapping follows the Conway era protocol_param_update CDDL:
///   0: min_fee_a, 1: min_fee_b, 2: max_block_body_size, 3: max_tx_size,
///   4: max_block_header_size, 5: key_deposit, 6: pool_deposit, 7: e_max,
///   8: n_opt, 9: a0, 10: rho, 11: tau, 13: min_pool_cost,
///   14: ada_per_utxo_byte, 15: cost_models, 16: execution_costs,
///   17: max_tx_ex_units, 18: max_block_ex_units, 19: max_val_size,
///   20: collateral_percentage, 21: max_collateral_inputs,
///   22: pool_voting_thresholds(5), 23: drep_voting_thresholds(10),
///   24: min_committee_size, 25: committee_term_limit, 26: gov_action_lifetime,
///   27: gov_action_deposit, 28: drep_deposit, 29: drep_activity,
///   30: min_fee_ref_script_cost_per_byte
pub(crate) fn encode_protocol_param_update(ppu: &ProtocolParamUpdate) -> Vec<u8> {
    // Count non-None fields to determine map size
    let mut entries: Vec<(u64, Vec<u8>)> = Vec::new();

    if let Some(v) = ppu.min_fee_a {
        entries.push((0, encode_uint(v)));
    }
    if let Some(v) = ppu.min_fee_b {
        entries.push((1, encode_uint(v)));
    }
    if let Some(v) = ppu.max_block_body_size {
        entries.push((2, encode_uint(v)));
    }
    if let Some(v) = ppu.max_tx_size {
        entries.push((3, encode_uint(v)));
    }
    if let Some(v) = ppu.max_block_header_size {
        entries.push((4, encode_uint(v)));
    }
    if let Some(ref v) = ppu.key_deposit {
        entries.push((5, encode_uint(v.0)));
    }
    if let Some(ref v) = ppu.pool_deposit {
        entries.push((6, encode_uint(v.0)));
    }
    if let Some(v) = ppu.e_max {
        entries.push((7, encode_uint(v)));
    }
    if let Some(v) = ppu.n_opt {
        entries.push((8, encode_uint(v)));
    }
    if let Some(ref v) = ppu.a0 {
        entries.push((9, encode_rational(v)));
    }
    if let Some(ref v) = ppu.rho {
        entries.push((10, encode_rational(v)));
    }
    if let Some(ref v) = ppu.tau {
        entries.push((11, encode_rational(v)));
    }
    // Key 12 is protocol_version — not in ProtocolParamUpdate (it's in HardForkInitiation)
    if let Some(ref v) = ppu.min_pool_cost {
        entries.push((13, encode_uint(v.0)));
    }
    if let Some(ref v) = ppu.ada_per_utxo_byte {
        entries.push((14, encode_uint(v.0)));
    }
    if let Some(ref v) = ppu.cost_models {
        entries.push((15, encode_cost_models(v)));
    }
    if let Some(ref v) = ppu.execution_costs {
        let mut buf = encode_array_header(2);
        buf.extend(encode_rational(&v.mem_price));
        buf.extend(encode_rational(&v.step_price));
        entries.push((16, buf));
    }
    if let Some(ref v) = ppu.max_tx_ex_units {
        let mut buf = encode_array_header(2);
        buf.extend(encode_uint(v.mem));
        buf.extend(encode_uint(v.steps));
        entries.push((17, buf));
    }
    if let Some(ref v) = ppu.max_block_ex_units {
        let mut buf = encode_array_header(2);
        buf.extend(encode_uint(v.mem));
        buf.extend(encode_uint(v.steps));
        entries.push((18, buf));
    }
    if let Some(v) = ppu.max_val_size {
        entries.push((19, encode_uint(v)));
    }
    if let Some(v) = ppu.collateral_percentage {
        entries.push((20, encode_uint(v)));
    }
    if let Some(v) = ppu.max_collateral_inputs {
        entries.push((21, encode_uint(v)));
    }
    // Key 22: pool_voting_thresholds — 5-element array
    if ppu.pvt_motion_no_confidence.is_some()
        || ppu.pvt_committee_normal.is_some()
        || ppu.pvt_committee_no_confidence.is_some()
        || ppu.pvt_hard_fork.is_some()
        || ppu.pvt_pp_security_group.is_some()
    {
        let mut buf = encode_array_header(5);
        let zero = Rational {
            numerator: 0,
            denominator: 1,
        };
        buf.extend(encode_rational(
            ppu.pvt_motion_no_confidence.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.pvt_committee_normal.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.pvt_committee_no_confidence.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(ppu.pvt_hard_fork.as_ref().unwrap_or(&zero)));
        buf.extend(encode_rational(
            ppu.pvt_pp_security_group.as_ref().unwrap_or(&zero),
        ));
        entries.push((22, buf));
    }
    // Key 23: drep_voting_thresholds — 10-element array
    if ppu.dvt_pp_network_group.is_some()
        || ppu.dvt_pp_economic_group.is_some()
        || ppu.dvt_pp_technical_group.is_some()
        || ppu.dvt_pp_gov_group.is_some()
        || ppu.dvt_hard_fork.is_some()
        || ppu.dvt_no_confidence.is_some()
        || ppu.dvt_committee_normal.is_some()
        || ppu.dvt_committee_no_confidence.is_some()
        || ppu.dvt_constitution.is_some()
        || ppu.dvt_treasury_withdrawal.is_some()
    {
        let mut buf = encode_array_header(10);
        let zero = Rational {
            numerator: 0,
            denominator: 1,
        };
        buf.extend(encode_rational(
            ppu.dvt_no_confidence.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_committee_normal.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_committee_no_confidence.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(ppu.dvt_hard_fork.as_ref().unwrap_or(&zero)));
        buf.extend(encode_rational(
            ppu.dvt_pp_network_group.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_pp_economic_group.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_pp_technical_group.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_pp_gov_group.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_treasury_withdrawal.as_ref().unwrap_or(&zero),
        ));
        buf.extend(encode_rational(
            ppu.dvt_constitution.as_ref().unwrap_or(&zero),
        ));
        entries.push((23, buf));
    }
    if let Some(v) = ppu.min_committee_size {
        entries.push((24, encode_uint(v)));
    }
    if let Some(v) = ppu.committee_term_limit {
        entries.push((25, encode_uint(v)));
    }
    if let Some(v) = ppu.gov_action_lifetime {
        entries.push((26, encode_uint(v)));
    }
    if let Some(ref v) = ppu.gov_action_deposit {
        entries.push((27, encode_uint(v.0)));
    }
    if let Some(ref v) = ppu.drep_deposit {
        entries.push((28, encode_uint(v.0)));
    }
    if let Some(v) = ppu.drep_activity {
        entries.push((29, encode_uint(v)));
    }
    if let Some(v) = ppu.min_fee_ref_script_cost_per_byte {
        entries.push((
            30,
            encode_rational(&Rational {
                numerator: v,
                denominator: 1,
            }),
        ));
    }

    let mut buf = encode_map_header(entries.len());
    for (key, value) in entries {
        buf.extend(encode_uint(key));
        buf.extend(value);
    }
    buf
}

/// Encode CostModels as CBOR map: {0: [v1...], 1: [v2...], 2: [v3...]}
pub(crate) fn encode_cost_models(cm: &CostModels) -> Vec<u8> {
    let count = [&cm.plutus_v1, &cm.plutus_v2, &cm.plutus_v3]
        .iter()
        .filter(|m| m.is_some())
        .count();
    let mut buf = encode_map_header(count);
    if let Some(ref v1) = cm.plutus_v1 {
        buf.extend(encode_uint(0));
        buf.extend(encode_array_header(v1.len()));
        for cost in v1 {
            buf.extend(encode_int(*cost as i128));
        }
    }
    if let Some(ref v2) = cm.plutus_v2 {
        buf.extend(encode_uint(1));
        buf.extend(encode_array_header(v2.len()));
        for cost in v2 {
            buf.extend(encode_int(*cost as i128));
        }
    }
    if let Some(ref v3) = cm.plutus_v3 {
        buf.extend(encode_uint(2));
        buf.extend(encode_array_header(v3.len()));
        for cost in v3 {
            buf.extend(encode_int(*cost as i128));
        }
    }
    buf
}
