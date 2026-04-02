# VRF Rejection Analysis — 2026-03-10

## Context

Commit `8c9b138` introduced exact 34-digit fixed-point taylorExpCmp VRF leader check
(replacing f64 approximation). The taylorExpCmp algorithm itself is CORRECT, but the
validation run exposed that **per-pool stake fractions fed into the check are wrong**,
causing valid live blocks to be rejected and the node to become stuck.

## Observed Failures

### Failure 1: pool1407hpuvtp9ww8s5mt53ear7062j463mvwhnypurlcask7djg3ae

```
WARN dugite_consensus::praos: Praos: VRF output does not satisfy leader eligibility threshold
  slot=106472524 relative_stake=8.797988584952467e-5
ERROR dugite_node::node: Consensus validation failed (strict): Not a slot leader — rejecting batch
  slot=106472524 block_no=4095271
```

- Block 4095271 confirmed accepted on-chain by Koios
- Dugite relative_stake: 8.8e-5 (0.0088%)
- Koios Set snapshot for this pool (epoch 1232): pool_stake=13,868,509,236,339 / active_stake=1,177,946,537,741,239 = **1.177%**
- Discrepancy: 133x too small

**Root cause of 133x discrepancy:**
`set_snapshot.pool_stake.values().sum()` in Dugite = ~157 quadrillion lovelace
Koios set snapshot active_stake = 1,177,946,537,741,239 (1.18 quadrillion)
Dugite's denominator is 133x too large → all fractions appear 133x too small.

This is likely caused by the double-replay issue: when the node restarts with a
ledger snapshot at slot 106449950 but a ChainDB tip at 106401541, it replays
1,500 blocks from the network ON TOP of a snapshot that already has those blocks
applied. This double-applies UTxO outputs (adding to stake_map twice) while
inputs from those blocks fail to subtract (UTxO set already consumed).

### Failure 2: pool1q65ag8panwayzaqfs6je7jz2ywt8x2032yaunq5hf25t7t8c26e

```
WARN dugite_consensus::praos: Praos: VRF output does not satisfy leader eligibility threshold
  slot=106473067 relative_stake=0.008867399057171464
ERROR dugite_node::node: Consensus validation failed (strict): Not a slot leader — rejecting batch
  slot=106473067 block_no=4095284
```

- Block 4095284 confirmed accepted on-chain by Koios
- Dugite relative_stake: 0.8867%
- Koios Set snapshot for this pool (epoch 1232): pool_stake=11,555,347,415,472 / active_stake=1,177,946,537,741,239 = **0.9810%**
- Discrepancy: Dugite underreports by ~10%

**Root cause of 10% discrepancy:**
Dugite's threshold = 1 - 0.95^0.008867 = 0.000454734
Koios actual threshold = 1 - 0.95^0.009810 = 0.000503047

The block's VRF leader_value landed in the gap [0.000455, 0.000503] — valid by Koios
threshold but just above Dugite's computed threshold. This is a stake accounting
error of ~10% in the set_snapshot.pool_stake for this pool.

## Chain Liveness Impact

After each VRF rejection:
1. The rejected block is NOT applied to ledger (node stays at previous tip hash)
2. Subsequent blocks from other peers have the rejected block as their parent
3. These blocks fail with "Block does not connect to tip: expected X, got Y"
4. The node is permanently stuck until the next rollback or restart

Observed stuck state: node stayed at hash `1ab1221732759c...` for all subsequent blocks
after rejecting block 4095284. The "does not connect" cascade is fatal.

## taylorExpCmp Algorithm Verification

The Python simulation confirms taylorExpCmp is correct:
- For clearly-eligible blocks (leader_value = threshold/2): returns Below ✓
- For boundary cases the math is sound

The exact arithmetic itself does NOT have a bug. The problem is the sigma value
fed into the check is wrong.

## Fix Recommendations

### Immediate Fix (Liveness): Disable strict VRF during sync

In `crates/dugite-node/src/node.rs` around line 1680:
```rust
// Current: strict=true causes rejection that sticks the node
if let Err(e) = self.consensus.validate_header_full(...) {
    if strict {
        error!(...);
        return;  // <--- THIS CAUSES STUCK NODE
    }
}
```

Consider: only enable strict mode when confirmed within a small window of true chain tip,
not during the "catching up from snapshot" phase where stake data may be stale.

OR: When a batch is rejected by VRF, attempt rollback to find alternate chain rather
than silently stopping (the cascade failure). The "does not connect" errors should
trigger a rollback attempt.

### Root Fix A: Fix double-replay stake inflation

When ledger snapshot tip > ChainDB tip, the node should either:
1. NOT replay blocks already applied in the snapshot (trim the sync start point)
2. Save ChainDB tip alongside the snapshot and replay from there instead

File: `crates/dugite-node/src/node.rs`
The intersection logic at line 1827 uses ChainDB tip when Ledger tip > ChainDB tip.
Blocks in range [ChainDB_tip, Ledger_tip] should NOT be re-applied to the ledger.

### Root Fix B: Correct ~10% stake undercount

The 10% undercount for pool1q65 has a different cause. With update query state sum ~0.95
and individual pools close to correct, the set_snapshot may be using stale data.

Investigate: does `update_query_state` (which computes from current live ledger state)
give different total than `set_snapshot.pool_stake.values().sum()`? The set snapshot
is 2 epochs old (mark at boundary-2, set at boundary-1, go at current).

If rewards are growing 10% per epoch and the set snapshot is one epoch behind,
this could explain a 10% undercount for pools with many delegators.

## Relevant Code Locations

- VRF check: `/Users/michaelfazio/Source/dugite/crates/dugite-consensus/src/praos.rs` lines 317-341
- Pool stake lookup: `/Users/michaelfazio/Source/dugite/crates/dugite-node/src/node.rs` lines 1626-1668
- Snapshot creation: `/Users/michaelfazio/Source/dugite/crates/dugite-ledger/src/state.rs` lines 1099-1136
- Exact VRF check: `/Users/michaelfazio/Source/dugite/crates/dugite-crypto/src/vrf.rs` lines 85-305
- Stake tracking (subtract): `state.rs` lines 680-688
- Stake tracking (add): `state.rs` lines 700-711

## Key Numbers (epoch 1232, preview testnet)

- Koios set snapshot total active_stake: 1,177,946,537,741,239 lovelace
- Dugite implied set snapshot total: ~157,632,725,962,600,544 lovelace (133x too large)
- update_query_state stake sum (current): reasonable (~0.95 of total)
- Dugite pool count: 653, delegation_count: 11,515
