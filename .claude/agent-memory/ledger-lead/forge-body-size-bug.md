---
name: forge-body-size-bug
description: Critical bugs found in block forging path — body_size miscalculation and epoch nonce at boundary
type: project
---

Two critical bugs found during forge.rs review (2026-03-20):

**Bug 1 (Critical): Wrong body_size in forged block headers**
- File: `crates/torsten-node/src/forge.rs:392–404` (`compute_body_size`)
- `compute_body_size` sums the CBOR size of each full transaction (`[body, witnesses, aux, is_valid]`), but `block_body_size` in the Cardano header must be the sum of the 4 separately-serialized block body components: `len(tx_bodies_array) + len(witnesses_array) + len(aux_map) + len(invalid_array)`.
- For empty blocks: returns 0, correct is 4 (four empty CBOR structures: `80 80 a0 80`).
- For non-empty blocks: roughly 2x wrong because witnesses are double-counted.
- This causes ALL forged blocks to be rejected by peers via `bbodySz` ledger rule.
- Fix: recompute body_size using the same 4-component serialization as `encode_block` and `compute_block_body_hash`.

**Bug 2 (High): Epoch nonce not updated at epoch boundaries when forging**
- File: `crates/torsten-node/src/node/mod.rs:2466`
- `let epoch_nonce = ls.epoch_nonce;` should be `ls.epoch_nonce_for_slot(next_slot.0)`.
- The sync path already uses `epoch_nonce_for_slot` correctly (`sync.rs:822`).
- Forging the first slot of a new epoch uses the prior epoch's nonce, causing VRF proof rejection.

**Bug 6 (Low): KES expiry off-by-one in forge.rs**
- File: `crates/torsten-node/src/forge.rs:334`
- `if kes_period_offset > MAX_KES_EVOLUTIONS` should be `>=`.
- Consensus path (`praos.rs:765`) correctly uses `>=`.

**Why:** The `body_size` bug is load-bearing — it blocks ALL block production from working. Must be fixed before soak testing with block forging enabled.

**How to apply:** When reviewing forge code or working on block production bugs, check these locations first.
