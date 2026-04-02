---
name: slow-path-rollback-utxo-store
description: Slow-path rollback was broken — it re-attached the stale pre-rollback UTxO store instead of restoring from the LSM snapshot
type: project
---

## The Bug (fixed in d3443a2)

When `handle_rollback` used the slow path (diff-based fast path couldn't cover the rollback depth), it:

1. Called `ls.detach_store()` to get the live UTxO store
2. Loaded the ledger snapshot: `*ls = snapshot_state`
3. Re-attached the live UTxO store: `ls.attach_utxo_store(old_store)`

This was fundamentally wrong. The live store contained UTxOs from blocks BEYOND the rollback point. After re-attaching it, replaying snapshot→rollback_slot would re-insert outputs but never remove the stale outputs from the rolled-back blocks. The UTxO store permanently diverged.

## The Fix

Open a fresh UTxO store from the "ledger" LSM snapshot, which reflects the exact UTxO set at snapshot_slot:

```rust
let utxo_store_path = self.database_path.join("utxo-store");
let restored_utxo_store = if utxo_store_path.exists() {
    match dugite_ledger::utxo_store::UtxoStore::open_from_snapshot(&utxo_store_path, "ledger") {
        Ok(mut store) => {
            store.count_entries();
            store.set_indexing_enabled(true);
            store.rebuild_address_index();
            Some(store)
        }
        Err(e) => { warn!(...); None }
    }
} else {
    None
};

if let Some(store) = restored_utxo_store {
    *ls = snapshot_state;
    ls.attach_utxo_store(store);
} else if utxo_store_path.exists() {
    // LSM store path exists but snapshot open failed → full genesis reset
    self.reset_ledger_and_replay(rollback_slot).await;
    return;
} else {
    // Pure in-memory mode: bincode snapshot already contains UTxO state
    *ls = snapshot_state;
}
// Then replay ApplyOnly from snapshot_slot to rollback_slot
```

## Key Invariant

The "ledger" LSM snapshot (`utxo-store/snapshots/ledger/`) and the bincode ledger snapshot (`ledger-snapshot.bin`) are always saved together by `save_utxo_snapshot()` + `save_snapshot()`. They are always in sync — both represent the UTxO state at the same slot. This sync is what makes the slow-path rollback correction possible.

## Warning

If no LSM snapshot exists AND the utxo_store_path exists (e.g., LSM store was just created but never snapshotted), the code falls back to `reset_ledger_and_replay` — a full genesis replay. This is expensive but correct.
