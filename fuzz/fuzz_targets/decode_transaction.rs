//! Fuzz target for `decode_transaction()` — transaction deserialization from CBOR.
//!
//! Run with: cargo +nightly fuzz run fuzz_decode_transaction -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Try all era variants — none should panic
    for era_id in 0..=6 {
        let _ = dugite_serialization::decode_transaction(era_id, data);
    }
});
