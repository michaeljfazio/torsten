//! Fuzz target for multiplexer segment parsing — the Ouroboros wire format parser.
//!
//! The multiplexer is the first code to touch raw TCP bytes from untrusted peers.
//! Segment::decode must handle any byte sequence without panicking.
//!
//! Run with: cargo +nightly fuzz run fuzz_mux_segment -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Segment::decode must never panic, regardless of input.
    // Valid segments return Ok((Segment, consumed_bytes)).
    // Invalid data returns Err(MuxError).
    let _ = torsten_network::multiplexer::Segment::decode(data);

    // If decode succeeds, verify the returned segment re-encodes without panicking
    if let Ok((segment, _consumed)) = torsten_network::multiplexer::Segment::decode(data) {
        let _ = segment.encode();
    }
});
