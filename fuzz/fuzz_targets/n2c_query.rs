//! Fuzz target for the N2C LocalStateQuery handler.
//!
//! Feeds random bytes as a CBOR query payload to `QueryHandler::handle_query_cbor`,
//! which is the primary entry point for untrusted query data from N2C clients.
//! The handler must never panic, regardless of the input.
//!
//! Run with: cargo +nightly fuzz run fuzz_n2c_query -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use torsten_network::QueryHandler;

fuzz_target!(|data: &[u8]| {
    // Create a minimal QueryHandler with default (empty) state.
    // The handler should gracefully handle any CBOR input without panicking,
    // returning an error variant or a default result.
    let handler = QueryHandler::new();

    // Test the unversioned query path (version 0, no gating)
    let _ = handler.handle_query_cbor(data);

    // Test versioned query paths with various N2C protocol versions.
    // Versions 16-22 are the supported N2C protocol versions.
    for version in [16_u16, 17, 18, 19, 20, 21, 22] {
        let _ = handler.handle_query_cbor_versioned(data, version);
    }
});
