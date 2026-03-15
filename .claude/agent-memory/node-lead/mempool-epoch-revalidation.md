---
name: mempool-epoch-revalidation
description: Pattern for epoch-boundary mempool revalidation against new protocol parameters
type: project
---

Implemented in T3-NEW-2. At epoch boundary (detected in `apply_blocks_batch` when `current_epoch > *last_snapshot_epoch`), after opcert counter pruning, call `mempool.revalidate_all()` with a closure that calls `torsten_ledger::validation::validate_transaction()` against the new epoch's protocol parameters.

**Why:** Protocol parameters change at epoch boundaries (fees, max tx size, execution unit prices). Transactions valid under old params may violate new ones. Haskell cardano-node does this. Critical for block producers to avoid forging invalid blocks.

**How to apply:** The revalidation block sits at the end of the `if current_epoch > *last_snapshot_epoch` block, after snapshot/opcert work. Borrows `utxo_set` directly from the ledger read-guard (no clone) to avoid memory pressure; clones only the small `protocol_params` and `slot_config` scalars for the closure.

**Location:** `crates/torsten-node/src/node.rs` lines ~3709-3752.

**Key pattern:**
- Hold ledger read lock across `revalidate_all` call (no deadlock — mempool locks are independent)
- `revalidate_all` snapshots the FIFO order before iterating, so `remove_tx` calls inside it are safe
- Logs `info!` with eviction count when any txs evicted; `debug!` when all pass
- Skipped entirely when mempool is empty (no lock acquisition needed)
