//! N2C LocalTxMonitor — snapshot-based mempool monitoring.
//!
//! Allows N2C clients to inspect the current mempool state via snapshots.
//! Each snapshot is immutable — subsequent queries see the same state even
//! if the mempool changes. MsgAwaitAcquire blocks until the mempool differs.
//!
//! ## Wire format
//! - `MsgDone` = `[0]`
//! - `MsgAcquire` = `[1]` (from StIdle)
//! - `MsgAcquired` = `[2, slot_no]`
//! - `MsgRelease` = `[3]`
//! - `MsgAwaitAcquire` = `[1]` (from StAcquired — same tag, different state)
//! - `MsgNextTx` = `[5]`
//! - `MsgReplyNextTx` = `[6, tx]` or `[6]`
//! - `MsgHasTx` = `[7, tx_id]`
//! - `MsgReplyHasTx` = `[8, bool]`
//! - `MsgGetSizes` = `[9]`
//! - `MsgReplyGetSizes` = `[10, [capacity, size, count]]`

pub mod server;
