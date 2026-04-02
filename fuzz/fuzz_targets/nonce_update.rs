//! Fuzz target for the evolving nonce update function.
//!
//! The nonce update combines the current evolving nonce with a VRF output
//! using blake2b_256. This fuzz target verifies that `blake2b_256` never
//! panics for arbitrary-length inputs and always produces a 32-byte hash.
//!
//! We cannot directly call `LedgerState::update_evolving_nonce` from the
//! fuzz harness (it requires constructing a full LedgerState), so instead
//! we exercise the core operation: blake2b_256(nonce || blake2b_256(eta)).
//!
//! Run with: cargo +nightly fuzz run fuzz_nonce_update -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use dugite_primitives::hash::blake2b_256;

fuzz_target!(|data: &[u8]| {
    // Simulate the nonce update: evolving' = blake2b_256(evolving || blake2b_256(eta))
    //
    // Split the fuzz input into two parts: a 32-byte "current nonce" prefix
    // and the remaining bytes as the VRF eta input.
    let (nonce_bytes, eta) = if data.len() >= 32 {
        (&data[..32], &data[32..])
    } else {
        // If input is too short, use zero nonce and the entire input as eta
        (&[0u8; 32][..], data)
    };

    // Step 1: hash the eta (matches update_evolving_nonce behavior)
    let eta_hash = blake2b_256(eta);

    // Step 2: concatenate nonce || eta_hash
    let mut combined = Vec::with_capacity(64);
    combined.extend_from_slice(nonce_bytes);
    combined.extend_from_slice(eta_hash.as_bytes());

    // Step 3: hash the combination
    let result = blake2b_256(&combined);

    // Must always produce exactly 32 bytes
    assert_eq!(result.as_bytes().len(), 32);
});
