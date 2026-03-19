//! Fuzz target for ConwayPParams positional array decoding.
//!
//! Protocol parameters are encoded as a positional CBOR array(31) per the
//! Haskell ConwayPParams EncCBOR format.  This target exercises the minicbor
//! decoder on the same positional array structure to ensure no panics on
//! arbitrary CBOR input.
//!
//! The expected wire format is:
//!   array(31) [
//!     min_fee_a, min_fee_b, max_block_body_size, max_tx_size,
//!     max_block_header_size, key_deposit, pool_deposit,
//!     e_max, n_opt, a0_rational, rho, tau,
//!     d (decentral_param), extra_entropy, protocol_version,
//!     min_pool_cost, ada_per_utxo_byte, cost_models,
//!     execution_costs, max_tx_ex_units, max_block_ex_units,
//!     max_val_size, collateral_percentage, max_collateral_inputs,
//!     pool_voting_thresholds, drep_voting_thresholds,
//!     min_committee_size, committee_max_term_length,
//!     governance_action_validity_period, governance_action_deposit,
//!     drep_deposit
//!   ]
//!
//! Run with: cargo +nightly fuzz run fuzz_protocol_params -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Try to decode a tagged rational [numerator, denominator]
fn try_decode_rational(decoder: &mut minicbor::Decoder<'_>) -> bool {
    match decoder.tag() {
        Ok(_) => {
            // Tagged rational: tag(30) [num, den]
            if let Ok(Some(2)) = decoder.array() {
                let _ = decoder.u64();
                let _ = decoder.u64();
                true
            } else {
                false
            }
        }
        Err(_) => {
            // Try as plain array [num, den]
            if let Ok(Some(2)) = decoder.array() {
                let _ = decoder.u64();
                let _ = decoder.u64();
                true
            } else {
                false
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // --- Test 1: Parse as positional array(31) ConwayPParams ---
    {
        let mut decoder = minicbor::Decoder::new(data);

        if let Ok(arr_len) = decoder.array() {
            let count = arr_len.unwrap_or(0);
            // Conway PParams has 31 fields; try to parse each one
            if count >= 31 {
                // Fields 0-4: min_fee_a, min_fee_b, max_block_body_size, max_tx_size,
                //             max_block_header_size (all integers)
                for _ in 0..5 {
                    if decoder.u64().is_err() {
                        return;
                    }
                }
                // Fields 5-6: key_deposit, pool_deposit
                for _ in 0..2 {
                    if decoder.u64().is_err() {
                        return;
                    }
                }
                // Field 7: e_max (epoch)
                let _ = decoder.u64();
                // Field 8: n_opt
                let _ = decoder.u64();
                // Field 9: a0 (rational)
                let _ = try_decode_rational(&mut decoder);
                // Field 10: rho (rational)
                let _ = try_decode_rational(&mut decoder);
                // Field 11: tau (rational)
                let _ = try_decode_rational(&mut decoder);
                // Remaining fields -- just skip them to exercise the decoder
                for _ in 12..count.min(64) {
                    if decoder.skip().is_err() {
                        break;
                    }
                }
            } else {
                // Short array -- skip all elements
                for _ in 0..count.min(64) {
                    if decoder.skip().is_err() {
                        break;
                    }
                }
            }
        }
    }

    // --- Test 2: Parse as a CBOR map (legacy pparam format with integer keys) ---
    {
        let mut decoder = minicbor::Decoder::new(data);

        if let Ok(map_len) = decoder.map() {
            let count = map_len.unwrap_or(0);
            for _ in 0..count.min(64) {
                // Key is an integer (0-33 for pparam fields)
                if decoder.u32().is_err() {
                    break;
                }
                // Value is variable type -- skip it
                if decoder.skip().is_err() {
                    break;
                }
            }
        }
    }

    // --- Test 3: Parse execution unit prices (nested structure) ---
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(Some(2)) = decoder.array() {
            // [priceMemory, priceSteps] where each is a tagged rational
            let _ = try_decode_rational(&mut decoder);
            let _ = try_decode_rational(&mut decoder);
        }
    }

    // --- Test 4: Parse cost models map ---
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(map_len) = decoder.map() {
            let count = map_len.unwrap_or(0);
            for _ in 0..count.min(8) {
                // Key: language version (integer 0=PlutusV1, 1=V2, 2=V3)
                if decoder.u32().is_err() {
                    break;
                }
                // Value: array of cost model parameters (integers)
                if let Ok(arr_len) = decoder.array() {
                    let param_count = arr_len.unwrap_or(0);
                    for _ in 0..param_count.min(512) {
                        if decoder.i64().is_err() {
                            break;
                        }
                    }
                } else {
                    break;
                }
            }
        }
    }
});
