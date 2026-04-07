//! Fuzz target for dugite-lsm operations.
//!
//! Interprets fuzz bytes as a sequence of LSM tree operations (insert, get,
//! delete, flush) and verifies correctness against a shadow HashMap.
//! Catches data loss, corruption, and panics under varied operation sequences.
//!
//! Run with: cargo +nightly fuzz run fuzz_lsm_operations -- -max_total_time=300

#![no_main]

use libfuzzer_sys::fuzz_target;
use std::collections::HashMap;

use dugite_lsm::{Key, LsmConfig, LsmTree, Value};

/// Operation types decoded from fuzz bytes.
enum Op {
    Insert { key: Vec<u8>, value: Vec<u8> },
    Get { key: Vec<u8> },
    Delete { key: Vec<u8> },
    Flush,
}

/// Parse fuzz bytes into a sequence of LSM operations.
/// Each operation consumes a control byte (top 2 bits = op, bottom 6 = key index)
/// plus optional value bytes.
fn parse_ops(data: &[u8], max_ops: usize) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut pos = 0;

    while pos < data.len() && ops.len() < max_ops {
        let control = data[pos];
        pos += 1;

        let op_type = control >> 6; // Top 2 bits: 0=insert, 1=get, 2=delete, 3=flush
        let key_idx = (control & 0x3F) as usize; // Bottom 6 bits: key selector

        // Use a small key space (0-63) to increase collision rate and exercise
        // overwrite/delete paths more frequently.
        let key = vec![key_idx as u8];

        match op_type {
            0 => {
                // Insert: read up to 16 bytes for the value
                let value_len = data.get(pos).copied().unwrap_or(0) as usize % 16 + 1;
                pos += 1;
                if pos > data.len() {
                    // No value bytes available — use the key as the value
                    ops.push(Op::Insert {
                        key: key.clone(),
                        value: key,
                    });
                } else {
                    let value_end = (pos + value_len).min(data.len());
                    let value = data[pos..value_end].to_vec();
                    pos = value_end;
                    ops.push(Op::Insert { key, value });
                }
            }
            1 => {
                ops.push(Op::Get { key });
            }
            2 => {
                ops.push(Op::Delete { key });
            }
            _ => {
                ops.push(Op::Flush);
            }
        }
    }

    ops
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    // Create a temp directory for the LSM tree
    let tempdir = match tempfile::tempdir() {
        Ok(d) => d,
        Err(_) => return,
    };

    // Use small config for fast fuzzing — tiny memtable forces frequent flushes
    let config = LsmConfig {
        memtable_size: 4096,
        block_cache_size: 8192,
        bloom_filter_bits_per_key: 10,
        wal_enabled: true,
        wal_segment_size: 4096,
        ..LsmConfig::default()
    };

    let mut tree = match LsmTree::open(tempdir.path(), config) {
        Ok(t) => t,
        Err(_) => return,
    };

    // Shadow map for correctness verification
    let mut shadow: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();

    let ops = parse_ops(data, 256);

    for op in ops {
        match op {
            Op::Insert { key, value } => {
                let lsm_key = Key::new(key.clone());
                let lsm_value = Value::new(value.clone());
                if tree.insert(&lsm_key, &lsm_value).is_ok() {
                    shadow.insert(key, value);
                }
            }
            Op::Get { key } => {
                let lsm_key = Key::new(key.clone());
                let result = tree.get(&lsm_key);
                if let Ok(maybe_val) = result {
                    let shadow_val = shadow.get(&key);
                    match (maybe_val.as_ref(), shadow_val) {
                        (Some(lsm_val), Some(expected)) => {
                            assert_eq!(
                                lsm_val.as_bytes(),
                                expected.as_slice(),
                                "LSM value mismatch for key {:?}",
                                key
                            );
                        }
                        (None, None) => {} // Both agree: key not present
                        (Some(lsm_val), None) => {
                            panic!(
                                "LSM returned value {:?} for key {:?} but shadow has no entry",
                                lsm_val.as_bytes(),
                                key
                            );
                        }
                        (None, Some(expected)) => {
                            panic!(
                                "LSM returned None for key {:?} but shadow has {:?}",
                                key, expected
                            );
                        }
                    }
                }
            }
            Op::Delete { key } => {
                let lsm_key = Key::new(key.clone());
                if tree.delete(&lsm_key).is_ok() {
                    shadow.remove(&key);
                }
            }
            Op::Flush => {
                let _ = tree.flush();
            }
        }
    }

    // Final consistency check: verify all shadow entries are present in the LSM tree
    for (key, expected_value) in &shadow {
        let lsm_key = Key::new(key.clone());
        if let Ok(Some(lsm_val)) = tree.get(&lsm_key) {
            assert_eq!(
                lsm_val.as_bytes(),
                expected_value.as_slice(),
                "Final check: value mismatch for key {:?}",
                key
            );
        } else {
            panic!(
                "Final check: key {:?} missing from LSM tree but present in shadow",
                key
            );
        }
    }
});
