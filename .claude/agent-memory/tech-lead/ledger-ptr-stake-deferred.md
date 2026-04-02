---
name: Deferred pointer stake (sisPtrStake) implementation
description: Haskell defers pointer address resolution to SNAP time; Dugite previously resolved eagerly causing 603 epoch mismatches from epoch 647
type: project
---

Haskell's `ShelleyInstantStake` has two separate buckets:
- `sisCredentialStake`: base/reward address coins — eagerly resolved to credential hash
- `sisPtrStake`: pointer address coins — deferred, resolved to credential via `saPtrs` at each SNAP boundary

Dugite previously resolved all addresses eagerly at UTxO insertion time and placed pointer coins in `stake_map`. This diverged whenever a credential deregistered after a pointer UTxO was created — Haskell excluded those coins from the snapshot, Dugite kept them in stake_map.

**Fix (implemented):**
- Added `ptr_stake: HashMap<Pointer, u64>` field to `LedgerState` with `#[serde(default)]`
- `StakeRouting` enum + `stake_routing()` helper in mod.rs replaces `stake_credential_hash_with_ptrs`
- All 4 UTxO mutation sites in apply.rs now route pointer outputs to `ptr_stake` and pointer inputs subtract from `ptr_stake`
- `process_epoch_transition` resolves `ptr_stake` at SNAP time: checks `pointer_map.get(ptr)`, `reward_accounts.contains_key(cred)`, `delegations.get(cred)` before adding to `pool_stake` and `snapshot_stake`
- `rebuild_stake_distribution` now populates both `stake_map` and `ptr_stake`
- `exclude_pointer_address_stake` (Conway HFC) now just clears `ptr_stake` — no UTxO scan needed
- `recompute_snapshot_pool_stakes` adds ptr_stake resolution using snapshot delegation maps

**Snapshot hash:** updated from `f61eb026...` to `9c9053e4...`

**Why:** Caused 603 epoch reward mismatches starting at epoch 647 on mainnet.

**How to apply:** When debugging epoch reward divergence between Dugite and Haskell, check whether ptr_stake is being resolved correctly at SNAP boundaries.
