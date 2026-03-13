//! ChainDB integration tests — full lifecycle operations.
//!
//! These tests exercise the complete write → flush → rollback → reopen lifecycle
//! that real node operations depend on.

use torsten_primitives::block::Point;
use torsten_primitives::hash::Hash32;
use torsten_primitives::time::{BlockNo, SlotNo};
use torsten_storage::chain_db::SECURITY_PARAM_K;
use torsten_storage::ChainDB;

/// Create a deterministic hash from a u64 block number.
fn make_hash(n: u64) -> Hash32 {
    let mut bytes = [0u8; 32];
    bytes[0..8].copy_from_slice(&n.to_be_bytes());
    Hash32::from_bytes(bytes)
}

/// Add a chain of N blocks to the ChainDB.
fn build_chain(db: &mut ChainDB, count: u64) {
    for i in 1..=count {
        db.add_block(
            make_hash(i),
            SlotNo(i * 10),
            BlockNo(i),
            make_hash(i - 1),
            format!("block-{i}").into_bytes(),
        )
        .unwrap();
    }
}

/// Test 1: Write N blocks to volatile, flush to immutable, verify all readable.
#[test]
fn test_write_flush_read_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = ChainDB::open(dir.path()).unwrap();

    let total = SECURITY_PARAM_K as u64 + 100;
    build_chain(&mut db, total);

    // Verify tip before flush
    assert_eq!(db.get_tip().block_number, BlockNo(total));

    // Flush finalized blocks (those deeper than k from tip)
    let flushed = db.flush_to_immutable().unwrap();
    assert_eq!(flushed, 100); // total - k = 100 blocks finalized

    // All blocks should still be readable (from either volatile or immutable)
    for i in 1..=total {
        let block = db.get_block(&make_hash(i)).unwrap();
        assert!(block.is_some(), "Block {i} should be readable after flush");
        assert_eq!(
            block.unwrap(),
            format!("block-{i}").into_bytes(),
            "Block {i} data mismatch"
        );
    }

    // Tip should be unchanged
    assert_eq!(db.get_tip().block_number, BlockNo(total));
}

/// Test 2: Rollback after flush — volatile blocks removed, immutable untouched.
#[test]
fn test_rollback_after_flush() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = ChainDB::open(dir.path()).unwrap();

    let total = SECURITY_PARAM_K as u64 + 200;
    build_chain(&mut db, total);

    // Flush 200 blocks to immutable
    let flushed = db.flush_to_immutable().unwrap();
    assert_eq!(flushed, 200);

    // Rollback to block (total - 50), removing 50 volatile blocks
    let rollback_to = total - 50;
    let removed = db
        .rollback_to_point(&Point::Specific(
            SlotNo(rollback_to * 10),
            make_hash(rollback_to),
        ))
        .unwrap();
    assert_eq!(removed.len(), 50);

    // Rolled-back blocks should be gone from volatile
    for i in (rollback_to + 1)..=total {
        assert!(
            !db.has_block(&make_hash(i)),
            "Block {i} should be removed after rollback"
        );
    }

    // Immutable blocks (1..=200) should still be intact
    for i in 1..=200u64 {
        assert!(
            db.has_block(&make_hash(i)),
            "Immutable block {i} should survive rollback"
        );
    }

    // Volatile blocks before rollback point should still exist
    for i in 201..=rollback_to {
        assert!(
            db.has_block(&make_hash(i)),
            "Volatile block {i} before rollback point should still exist"
        );
    }
}

/// Test 3: Crash recovery — write blocks, drop ChainDB, reopen, verify immutable tip.
#[test]
fn test_crash_recovery_reopen() {
    let dir = tempfile::tempdir().unwrap();

    // Phase 1: write blocks and flush some to immutable
    {
        let mut db = ChainDB::open(dir.path()).unwrap();
        let total = SECURITY_PARAM_K as u64 + 50;
        build_chain(&mut db, total);

        let flushed = db.flush_to_immutable().unwrap();
        assert_eq!(flushed, 50);

        // Persist to ensure immutable data is on disk
        db.persist().unwrap();
    }
    // ChainDB dropped here — simulates crash

    // Phase 2: reopen and verify immutable data survived
    {
        let db = ChainDB::open(dir.path()).unwrap();

        // Immutable blocks should be readable
        for i in 1..=50u64 {
            let block = db.get_block(&make_hash(i)).unwrap();
            assert!(block.is_some(), "Immutable block {i} should survive reopen");
            assert_eq!(
                block.unwrap(),
                format!("block-{i}").into_bytes(),
                "Immutable block {i} data mismatch after reopen"
            );
        }

        // Volatile blocks are lost on restart (expected behavior)
        // The tip should reflect only immutable data
        assert!(db.has_immutable());
    }
}

/// Test 4: Batch write of 1000 blocks and verify all stored.
#[test]
fn test_batch_write_1000_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = ChainDB::open(dir.path()).unwrap();

    let count = 1000u64;
    let blocks: Vec<_> = (1..=count)
        .map(|i| {
            (
                make_hash(i),
                SlotNo(i * 10),
                BlockNo(i),
                make_hash(i - 1),
                format!("batch-block-{i}").into_bytes(),
            )
        })
        .collect();

    db.add_blocks_batch(blocks).unwrap();

    // Verify all 1000 blocks stored
    for i in 1..=count {
        assert!(db.has_block(&make_hash(i)), "Batch block {i} should exist");
        let data = db.get_block(&make_hash(i)).unwrap().unwrap();
        assert_eq!(data, format!("batch-block-{i}").into_bytes());
    }

    // Tip should be the last block
    assert_eq!(db.get_tip().block_number, BlockNo(count));
    assert_eq!(db.tip_slot(), SlotNo(count * 10));
}
