//! Fuzz target for N2N/N2C handshake version proposal parsing.
//!
//! The handshake is the very first protocol exchange on every connection.
//! Both the client path (parsing MsgAcceptVersion/MsgRefuse) and the server
//! path (parsing MsgProposeVersions) use minicbor to decode CBOR from the
//! wire.  This target exercises those same CBOR parsing patterns to ensure
//! no panics on arbitrary input.
//!
//! Wire formats:
//!   MsgProposeVersions = [0, { version: [magic, ...params], ... }]
//!   MsgAcceptVersion   = [1, version, [magic, query]]
//!   MsgRefuse          = [2, reason]
//!
//! Run with: cargo +nightly fuzz run fuzz_handshake -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Exercise the CBOR decoder on the same patterns the handshake parsers use.
    // None of these should panic, regardless of input.

    // --- Server path: parsing MsgProposeVersions [0, { version: params }] ---
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(Some(_arr_len)) = decoder.array() {
            if let Ok(msg_tag) = decoder.u32() {
                if msg_tag == 0 {
                    // Try to parse the version map
                    if let Ok(map_len) = decoder.map() {
                        let count = map_len.unwrap_or(0);
                        for _ in 0..count.min(64) {
                            // Parse version number
                            if decoder.u32().is_err() {
                                break;
                            }
                            // Try to extract network magic from params array
                            let pos = decoder.position();
                            if let Ok(Some(_param_arr_len)) = decoder.array() {
                                let _ = decoder.u64(); // network magic
                            }
                            decoder.set_position(pos);
                            // Skip the full value
                            if decoder.skip().is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Client path: parsing MsgAcceptVersion [1, version, [magic, query]] ---
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(Some(_arr_len)) = decoder.array() {
            if let Ok(msg_tag) = decoder.u32() {
                match msg_tag {
                    1 => {
                        // MsgAcceptVersion: [1, version, params]
                        let _ = decoder.u32(); // version
                        if let Ok(Some(_param_len)) = decoder.array() {
                            let _ = decoder.u64(); // network magic
                            let _ = decoder.bool(); // query mode
                        }
                    }
                    2 => {
                        // MsgRefuse: [2, reason]
                        // reason is [tag, ...] where tag is 0 (VersionMismatch),
                        // 1 (HandshakeDecodeError), or 2 (Refused)
                        if let Ok(Some(_reason_len)) = decoder.array() {
                            let _ = decoder.u32(); // reason tag
                            let _ = decoder.skip(); // reason payload
                        }
                    }
                    _ => {
                        // Unknown message tag -- skip
                        let _ = decoder.skip();
                    }
                }
            }
        }
    }

    // --- N2C handshake with bit-15 version encoding ---
    // N2C versions have bit 15 set on the wire (e.g., 16 -> 0x8010)
    {
        let mut decoder = minicbor::Decoder::new(data);
        if let Ok(Some(_arr_len)) = decoder.array() {
            if let Ok(msg_tag) = decoder.u32() {
                if msg_tag == 0 {
                    if let Ok(map_len) = decoder.map() {
                        let count = map_len.unwrap_or(0);
                        for _ in 0..count.min(64) {
                            if let Ok(wire_version) = decoder.u32() {
                                // Strip bit 15 for N2C version
                                let _version = wire_version & 0x7FFF;
                            } else {
                                break;
                            }
                            if decoder.skip().is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
});
