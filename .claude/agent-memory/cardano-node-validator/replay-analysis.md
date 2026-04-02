# Full Ledger Replay Analysis (2026-03-10, commit 688c30e)

## Test Conditions
- DB: `./db-preview/` (Mithril snapshot, 4,093,131 blocks, slot ~106.4M)
- Ledger snapshot deleted before start (`rm -f ./db-preview/ledger-snapshot.bin`)
- DUGITE_REPLAY_LIMIT: unset (default = u64::MAX = replay all)
- Command: `DUGITE_PIPELINE_DEPTH=150 ./target/release/dugite-node run ...`

## Replay Performance
- Total blocks replayed: 4,093,131
- Wall clock time: 103 seconds
- Average speed: 39,549 blocks/sec
- Peak speed: ~39,290 blocks/sec (near end)
- Early speed: ~31,584 blocks/sec (at 9.76%)
- Progress logged every 5 seconds

Sample progress log:
```
Replaying 9.76%  | slot 10382760/106401541  | 473760 blocks  | 31584 blocks/s | 65 UTxOs
Replaying 39.66% | slot 42202459/106401541  | 1810597 blocks | 36175 blocks/s | 98 UTxOs
Replaying 73.90% | slot 78628447/106401541  | 3162559 blocks | 37180 blocks/s | 148 UTxOs
Replaying 95.49% | slot 101606464/106401541 | 3940017 blocks | 39290 blocks/s | 149 UTxOs
Ledger replay from local storage complete: replayed=4093131 elapsed_secs=103 speed=39549
```

## UTxO Count Anomaly
UTxO count at various progress points:
- 9.76% (473k blocks):  65 UTxOs
- 39.66% (1.8M blocks): 98 UTxOs
- 51.33% (2.3M blocks): 110 UTxOs
- 65.37% (2.8M blocks): 146 UTxOs
- 80.10% (3.4M blocks): 149 UTxOs
- 95.49% (3.9M blocks): 149 UTxOs
- 100% (4.09M blocks):  149 UTxOs (in snapshot)

Expected: hundreds of thousands of UTxOs for preview testnet.

### Root Cause
In `crates/dugite-ledger/src/state.rs` lines 623-632:
```rust
if let Err(e) = self.utxo_set.apply_transaction(&tx.hash, &tx.body.inputs, &tx.body.outputs) {
    // During initial sync without full history, inputs won't be found.
    // Skip UTxO changes entirely to avoid phantom outputs...
    debug!("UTxO application skipped (missing inputs): {e}");
}
```

`apply_transaction()` in `crates/dugite-ledger/src/utxo.rs` lines 51-78:
- Validates ALL inputs exist BEFORE removing any or adding outputs
- Returns Err if ANY input is missing
- Mithril import starts from mid-chain with empty UTxO set
- First transaction's inputs are not in set → entire tx skipped (no outputs added)
- Cascading: no UTxOs ever accumulate → all subsequent transactions also fail
- Only 149 UTxOs come from genesis-era transactions where inputs happen to be present

### Fix Options
1. **Seed genesis UTxOs**: Load bootstrap UTxOs from genesis files before replay
2. **Partial apply**: When inputs are missing, still add outputs (accept phantom credits)
   - Note: this would give wrong stake distribution but correct UTxO count
3. **Full chain from genesis**: Only viable by downloading from genesis, not Mithril
4. **Mithril UTxO snapshot**: Import UTxO set from Mithril along with blocks (requires Mithril UTxO service)

## Stake Distribution Anomaly

### Root Cause
Stake credits (from outputs) fire at lines 634-641 OUTSIDE the `apply_transaction` result check.
But stake debits (from inputs) at lines 612-621 use `utxo_set.lookup()` which returns None for
all inputs (set is empty). So:
- Credits: added for every output in every transaction
- Debits: never subtracted (lookup fails)

Result: stake accumulates without decay → some credentials have inflated stake, never reduced.

### Observed Comparison (Set snapshot, epoch 1232)
| Pool | Dugite σ | Koios σ | Ratio |
|------|-----------|---------|-------|
| pool1xgmqwh23 | 0.004254 (0.43%) | 0.04509 (4.51%) | 10x too low |
| pool1fw7yf4me | 0.020433 (2.04%) | 0.00147 (0.15%) | 13x too high |

The discrepancy goes in BOTH directions, confirming the credits-without-debits model breaks
the stake accounting in complex ways.

### Epoch Nonce Mismatch (Secondary Issue)
- Dugite epoch 1232 nonce: `68727533dd7ba820be27e194df11bc20395b9f0d41d5f3c57c0e439749476a3d`
- Koios Set snapshot nonce: `737c9befe36e706842fcb38245f15807b6a7763fd3825108df007f9c145fdcf1`
- These differ, meaning the VRF inputs used for leader eligibility checks are wrong
- Even if stake distribution were fixed, the epoch nonce mismatch would still cause VRF failures

## Protocol Parameter Updates
Only 3 pre-Conway PP updates applied (epochs 21, 106, 359):
```
Pre-Conway protocol parameter update applied epoch=21  proposers=7
Pre-Conway protocol parameter update applied epoch=106 proposers=7
Pre-Conway protocol parameter update applied epoch=359 proposers=7
```

Conway governance ratification events fired:
```
2 governance proposal(s) ratified and enacted at epoch 1227
1 governance proposal(s) ratified and enacted at epoch 1228
1 governance proposal(s) ratified and enacted at epoch 1229
```

But the enacted governance actions did not update protocol parameters.

Final state (Dugite) vs actual (Koios epoch 1232):
| Parameter | Dugite | Actual |
|-----------|---------|--------|
| maxBlockBodySize | 65536 | 90112 |
| protocolVersion | {8, 0} | {10, 0} |
| committeeMinSize | 0 | 3 |
| maxTxExecutionUnits.memory | 10M | 16.5M |
| maxBlockExecutionUnits.memory | 62M | 72M |

Investigation needed: are the ratified proposals PParamsUpdate actions? Check what GovActionId types they are.

## Post-Replay Sync Behavior
After replay completes:
1. Query state initialized from ledger
2. Metrics/N2C/N2N servers start
3. Peers connect (5 peers in ~3 seconds)
4. Pipelined sync catches up remaining ~700 blocks at 182 b/s
5. Chain intersection found at slot 106401541
6. Epoch transition 1231->1232 during catchup sync (correct)
7. First live block arrives → REJECTED (VRF strict mode)
8. Subsequent blocks: "does not connect to tip" ERROR (ledger frozen at 106457803)

The node becomes completely non-functional at chain tip after full replay.
All blocks after slot 106457803 are rejected, node ledger advances no further.
