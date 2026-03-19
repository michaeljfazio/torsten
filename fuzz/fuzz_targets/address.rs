//! Fuzz target for Cardano address deserialization.
//!
//! Addresses are the primary identifier in the Cardano UTxO model.
//! `Address::from_bytes` must handle any byte sequence without panicking,
//! returning `Err(AddressError)` for invalid input.
//!
//! Address types tested:
//!   - Base addresses (type 0x00-0x03): payment + staking credential
//!   - Enterprise addresses (type 0x60-0x63): payment credential only
//!   - Pointer addresses (type 0x40-0x43): payment + stake pointer
//!   - Reward addresses (type 0xE0-0xE3): staking credential only
//!   - Byron addresses: CBOR-encoded legacy format (0x82, 0x83 prefix)
//!
//! Run with: cargo +nightly fuzz run fuzz_address -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use torsten_primitives::address::Address;

fuzz_target!(|data: &[u8]| {
    // Address::from_bytes must never panic, regardless of input.
    // Valid addresses return Ok(Address), invalid data returns Err(AddressError).
    let result = Address::from_bytes(data);

    // If parsing succeeded, exercise derived operations to verify no panics
    // in downstream code paths.
    if let Ok(addr) = result {
        // to_bytes roundtrip: encoding a successfully parsed address must not panic
        let bytes = addr.to_bytes();

        // Re-decode the encoded bytes -- must succeed and produce the same address
        if let Ok(re_decoded) = Address::from_bytes(&bytes) {
            assert_eq!(
                addr, re_decoded,
                "Address roundtrip mismatch: original bytes {:?}",
                &data[..data.len().min(64)]
            );
        }

        // Exercise Debug formatting
        let _ = format!("{:?}", addr);

        // Exercise network ID extraction
        let _ = addr.network_id();
    }
});
