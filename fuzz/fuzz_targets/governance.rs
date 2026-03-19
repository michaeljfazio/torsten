//! Fuzz target for CIP-0129 governance identifier bech32 decoding.
//!
//! Governance identifiers (DRep credentials, CC hot/cold credentials) are
//! bech32-encoded 28-byte hashes. The decode functions must handle any
//! string input without panicking, returning appropriate errors for invalid data.
//!
//! Additionally, this target exercises CBOR-based governance action/vote
//! decoding patterns via minicbor.
//!
//! Run with: cargo +nightly fuzz run fuzz_governance -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // --- Test 1: Bech32 governance identifier decoding ---
    // Try interpreting the fuzz input as a UTF-8 string for bech32 decoding.
    if let Ok(s) = std::str::from_utf8(data) {
        // None of these should panic, regardless of input string
        let _ = torsten_primitives::governance::decode_drep_key(s);
        let _ = torsten_primitives::governance::decode_drep_script(s);
        let _ = torsten_primitives::governance::decode_cc_hot_key(s);
        let _ = torsten_primitives::governance::decode_cc_hot_script(s);
        let _ = torsten_primitives::governance::decode_cc_cold_key(s);
        let _ = torsten_primitives::governance::decode_cc_cold_script(s);
    }

    // --- Test 2: Governance action CBOR parsing ---
    // Conway governance actions are encoded as CBOR arrays:
    //   [action_type, ...params]
    // Action types: 0=ParameterChange, 1=HardForkInitiation,
    //   2=TreasuryWithdrawals, 3=NoConfidence, 4=UpdateCommittee,
    //   5=NewConstitution, 6=InfoAction
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(arr_len) = decoder.array() {
            let count = arr_len.unwrap_or(0);
            if count >= 1 {
                if let Ok(action_type) = decoder.u32() {
                    match action_type {
                        0 => {
                            // ParameterChange: [0, gov_action_id?, protocol_param_update, policy_hash?]
                            for _ in 1..count.min(8) {
                                if decoder.skip().is_err() {
                                    break;
                                }
                            }
                        }
                        1 => {
                            // HardForkInitiation: [1, gov_action_id?, protocol_version]
                            for _ in 1..count.min(8) {
                                if decoder.skip().is_err() {
                                    break;
                                }
                            }
                        }
                        2 => {
                            // TreasuryWithdrawals: [2, { reward_acct: coin }, policy_hash?]
                            if let Ok(map_len) = decoder.map() {
                                let mcount = map_len.unwrap_or(0);
                                for _ in 0..mcount.min(64) {
                                    let _ = decoder.bytes(); // reward account
                                    let _ = decoder.u64(); // coin amount
                                }
                            }
                        }
                        3 => {
                            // NoConfidence: [3, gov_action_id?]
                            let _ = decoder.skip();
                        }
                        4 => {
                            // UpdateCommittee: [4, gov_action_id?, remove_set, add_map, threshold]
                            for _ in 1..count.min(8) {
                                if decoder.skip().is_err() {
                                    break;
                                }
                            }
                        }
                        5 => {
                            // NewConstitution: [5, gov_action_id?, constitution]
                            for _ in 1..count.min(8) {
                                if decoder.skip().is_err() {
                                    break;
                                }
                            }
                        }
                        6 => {
                            // InfoAction: [6]
                            // No additional fields
                        }
                        _ => {
                            // Unknown action type -- skip remaining
                            for _ in 1..count.min(16) {
                                if decoder.skip().is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Test 3: Vote CBOR parsing ---
    // Votes are encoded as: [voter, gov_action_id, vote, anchor?]
    // voter = [voter_type, credential]
    // gov_action_id = [tx_hash, index]
    // vote = 0 (No) | 1 (Yes) | 2 (Abstain)
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(arr_len) = decoder.array() {
            let count = arr_len.unwrap_or(0);
            if count >= 3 {
                // voter = [type, credential]
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.u32(); // voter type
                    let _ = decoder.bytes(); // credential hash
                }
                // gov_action_id = [tx_hash, index]
                if let Ok(Some(2)) = decoder.array() {
                    let _ = decoder.bytes(); // tx hash
                    let _ = decoder.u32(); // index
                }
                // vote
                let _ = decoder.u32();
                // optional anchor
                if count >= 4 {
                    let _ = decoder.skip();
                }
            }
        }
    }
});
