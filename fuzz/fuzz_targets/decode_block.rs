//! Fuzz target for `decode_block()` — the primary untrusted input parser from the network.
//!
//! Run with: cargo +nightly fuzz run fuzz_decode_block -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Should never panic, regardless of input
    let _ = dugite_serialization::decode_block(data);
});
