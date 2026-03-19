//! Fuzz target for ChainSync protocol message decoding.
//!
//! ChainSync messages are CBOR-encoded arrays where the first element is a
//! message tag identifying the type:
//!
//!   [0]                         — MsgRequestNext
//!   [1]                         — MsgAwaitReply
//!   [2, header/block, tip]      — MsgRollForward
//!   [3, point, tip]             — MsgRollBackward
//!   [4, points]                 — MsgFindIntersect
//!   [5, point, tip]             — MsgIntersectFound
//!   [6, tip]                    — MsgIntersectNotFound
//!   [7]                         — MsgDone
//!
//! The point type is either [] (Origin) or [slot, hash].
//! The tip type is [point, block_no].
//!
//! This target exercises minicbor decoding of all ChainSync message patterns
//! to verify they never panic on arbitrary input.
//!
//! Run with: cargo +nightly fuzz run fuzz_chainsync_msg -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;

/// Attempt to decode a Point from the current decoder position.
/// Returns true if parsing succeeded (or gracefully failed), false on panic-worthy error.
fn try_decode_point(decoder: &mut minicbor::Decoder<'_>) -> bool {
    match decoder.array() {
        Ok(Some(0)) => true, // Origin = []
        Ok(Some(2)) => {
            // Specific = [slot, hash]
            let _ = decoder.u64();
            let _ = decoder.bytes();
            true
        }
        Ok(Some(n)) => {
            // Unknown length -- skip remaining elements
            for _ in 0..n.min(16) {
                if decoder.skip().is_err() {
                    return false;
                }
            }
            true
        }
        Ok(None) => {
            // Indefinite-length array -- skip elements until break
            loop {
                match decoder.skip() {
                    Ok(()) => {}
                    Err(_) => return false,
                }
                // Try to detect end of indefinite array
                if decoder.position() >= decoder.input().len() {
                    return false;
                }
            }
        }
        Err(_) => false,
    }
}

/// Attempt to decode a Tip = [point, block_no] from the current decoder position.
fn try_decode_tip(decoder: &mut minicbor::Decoder<'_>) -> bool {
    match decoder.array() {
        Ok(Some(2)) => {
            if !try_decode_point(decoder) {
                return false;
            }
            let _ = decoder.u64(); // block_no
            true
        }
        Ok(Some(n)) => {
            for _ in 0..n.min(16) {
                if decoder.skip().is_err() {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

fuzz_target!(|data: &[u8]| {
    let mut decoder = minicbor::Decoder::new(data);

    // Parse the outer array and message tag
    let arr_len = match decoder.array() {
        Ok(len) => len,
        Err(_) => return,
    };

    let msg_tag = match decoder.u32() {
        Ok(tag) => tag,
        Err(_) => return,
    };

    // Dispatch on message tag -- exercise all ChainSync message patterns
    match msg_tag {
        0 => {
            // MsgRequestNext = [0]
            // No additional fields
        }
        1 => {
            // MsgAwaitReply = [1]
            // No additional fields
        }
        2 => {
            // MsgRollForward = [2, header/block, tip]
            // The header/block is an opaque CBOR value (wrapped block)
            let _ = decoder.skip(); // header or block
            try_decode_tip(&mut decoder);
        }
        3 => {
            // MsgRollBackward = [3, point, tip]
            try_decode_point(&mut decoder);
            try_decode_tip(&mut decoder);
        }
        4 => {
            // MsgFindIntersect = [4, [point, ...]]
            if let Ok(points_len) = decoder.array() {
                let count = points_len.unwrap_or(0);
                for _ in 0..count.min(64) {
                    if !try_decode_point(&mut decoder) {
                        break;
                    }
                }
            }
        }
        5 => {
            // MsgIntersectFound = [5, point, tip]
            try_decode_point(&mut decoder);
            try_decode_tip(&mut decoder);
        }
        6 => {
            // MsgIntersectNotFound = [6, tip]
            try_decode_tip(&mut decoder);
        }
        7 => {
            // MsgDone = [7]
            // No additional fields
        }
        _ => {
            // Unknown tag -- skip remaining elements
            if let Some(len) = arr_len {
                for _ in 1..len.min(16) {
                    if decoder.skip().is_err() {
                        break;
                    }
                }
            }
        }
    }
});
