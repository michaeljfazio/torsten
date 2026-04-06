# Proptest Expansion Design — Issue #342

**Date:** 2026-04-06
**Issue:** #342 — Expand proptest coverage for epoch transitions, UTxO invariants, and mempool

## Overview

Add 27 new property-based tests across 4 domains: epoch transitions (7), UTxO invariants (7), mempool invariants (7), and protocol parameter transitions (6). Full state machine testing approach — generate complete LedgerState instances with pools, delegations, UTxO sets, and governance state, then verify invariants through transitions.

## File Organization

| File | Crate | Cases | Properties |
|------|-------|-------|------------|
| `tests/strategies.rs` | dugite-ledger | — | Shared generators |
| `tests/epoch_proptest.rs` | dugite-ledger | 256 | 7 |
| `tests/utxo_proptest.rs` | dugite-ledger | 256 | 7 |
| `tests/mempool_proptest.rs` | dugite-mempool | 1000 | 7 |
| `tests/protocol_params_proptest.rs` | dugite-ledger | 1000 | 6 |

## Shared Strategy Module

`dugite-ledger/tests/strategies.rs`

### Core Generators

- `arb_pool_id()` → random Hash28 pool ID
- `arb_pool_params()` → PoolParams with random cost/margin/pledge/relays
- `arb_stake_credential()` → VerificationKey or Script credential
- `arb_delegation(pools)` → random credential→pool mapping
- `arb_reward_account()` → reward address with random credential
- `arb_lovelace(min, max)` → bounded Lovelace value
- `arb_rational()` → Rat with bounded numerator/denominator

### Composite Generators

- `arb_utxo_entry()` → (TransactionInput, TransactionOutput) pair
- `arb_utxo_set(n)` → UtxoSet with n random entries, all values > 0
- `arb_protocol_params()` → ProtocolParameters with valid field ranges
- `arb_drep()` → DRep registration with random metadata
- `arb_stake_snapshot(pools, delegations)` → StakeSnapshot consistent with given pools/delegations

### State Generators

- `arb_ledger_state(config)` → Full LedgerState with:
  - 1–20 registered pools with random params
  - 1–100 delegations across those pools
  - 100–1000 UTxO entries
  - Treasury/reserves that sum correctly with UTxO total
  - 0–10 DRep registrations
  - Valid protocol parameters
  - Consistent snapshot state (mark/set/go)

The `config` parameter controls bounds (pool count range, delegation count, UTxO count) so each test domain can tune generation to its needs.

## Epoch Transition Tests

`dugite-ledger/tests/epoch_proptest.rs` — 256 cases per property.

### Properties

1. **Rewards are non-negative** — Generate ledger state at epoch boundary, run `process_epoch_transition()`, verify every reward in the update is >= 0. No pool or delegator receives a negative reward.

2. **Total ADA conservation** — Before and after transition: `utxo_total + reserves + treasury + reward_accounts + deposits == max_lovelace`. The total supply is invariant across epoch boundaries.

3. **Snapshot rotation correctness** — At epoch N, verify: previous `mark` becomes `set`, previous `set` becomes `go`, and a new `mark` is computed from current stake distribution. Compare snapshots structurally.

4. **Pool retirement processing** — Register pools with `retiring_epoch <= current_epoch`, run transition, verify: retired pools removed from pool_params, their delegators' stake returns to reward accounts, pool deposits refunded.

5. **Reward distribution proportionality** — For a simple scenario (single pool, multiple delegators), verify each delegator's reward is proportional to their stake share within that pool (within rounding tolerance of 1 lovelace).

6. **Protocol parameter update activation** — Enqueue a parameter update proposal for epoch N, run transition to epoch N, verify the new params are active. Run transition to N+1, verify they persist (not reverted).

7. **Idempotent epoch detection** — Calling `process_epoch_transition()` with the same epoch number twice doesn't double-apply rewards or rotate snapshots twice.

## UTxO Invariant Tests

`dugite-ledger/tests/utxo_proptest.rs` — 256 cases per property.

### Properties

1. **Conservation after valid block** — Generate a UTxO set and a valid transaction (inputs exist, outputs + fees = inputs). Apply block. Verify: `sum(consumed_inputs) == sum(new_outputs) + fees`. For minting txs: include mint quantity in the balance.

2. **No zero-value UTxO entries** — After applying any sequence of 1–10 valid blocks, scan the entire UTxO set. No entry has a coin value of 0.

3. **Rollback restores exact state** — Generate UTxO set, snapshot it, apply a valid block, then rollback. Verify the UTxO set is byte-identical to the snapshot. Test with both single blocks and chains of 1–5 blocks.

4. **DiffSeq capacity invariant** — Apply blocks until DiffSeq reaches capacity `k`. Verify it holds exactly `k` diffs. Apply one more block, verify it still holds exactly `k` (oldest diff evicted). Never exceeds `k`.

5. **DiffSeq rollback consistency** — Apply N blocks (N <= k), rollback M blocks (M <= N). Verify the UTxO set matches the state after applying only the first N-M blocks.

6. **Input consumption is atomic** — After a valid block with multiple transactions, every consumed input is absent from the UTxO set, and every produced output is present. No partial application.

7. **Duplicate input rejection** — Generate a transaction that references the same input twice. Verify validation rejects it before it touches the UTxO set.

## Mempool Invariant Tests

`dugite-mempool/tests/mempool_proptest.rs` — 1000 cases per property.

### Properties

1. **No duplicate transaction IDs** — Add 10–50 random transactions. After each add, verify all tx IDs in the mempool are unique.

2. **Size limit enforcement** — Configure mempool with `max_transactions = N`. Add more than N transactions. Verify `mempool.len() <= N` at all times. Same for `max_bytes` — total serialized size never exceeds the limit.

3. **TTL sweep completeness** — Add transactions with random TTL values. Advance time past some TTLs, run sweep. Verify: no transaction with expired TTL remains, all non-expired transactions are still present.

4. **Input conflict detection** — Generate two transactions that spend the same input. Add the first (succeeds). Add the second — verify it's rejected. The first transaction remains unaffected.

5. **Removal frees inputs** — Add a transaction spending input X. Remove it. Add a new transaction spending input X. Verify the second add succeeds — the input is available again.

6. **Fee density ordering** — Add transactions with varying fee/size ratios. Verify iteration order respects fee density (highest first). Property: for any adjacent pair in iteration, `fee_a / size_a >= fee_b / size_b`.

7. **Dual-FIFO fairness** — Add a mix of Local and Remote origin transactions. Verify both origins are represented in the mempool (neither origin starves the other when capacity allows).

## Protocol Parameter Transition Tests

`dugite-ledger/tests/protocol_params_proptest.rs` — 1000 cases per property.

### Properties

1. **Parameter bounds enforcement** — Generate random ProtocolParameters. Verify: `min_fee_a >= 0`, `min_fee_b >= 0`, `max_block_size > 0`, `max_tx_size > 0`, `max_block_ex_units.mem > 0`, `max_block_ex_units.steps > 0`, all governance thresholds in [0, 1]. Any violation is caught by validation.

2. **Update quorum rules** — Generate a parameter update proposal with N signers out of M genesis delegates. Verify: accepted iff `N >= quorum_threshold`, rejected otherwise.

3. **Era-specific parameter presence** — For each era (Shelley through Conway), generate params and verify era-specific fields: Shelley has no `collateral_percentage`, Alonzo adds execution costs, Babbage adds ref script costs, Conway adds governance thresholds (DRep/SPO/CC voting). Missing fields for the era are None/default.

4. **Update preserves unchanged fields** — Apply a partial parameter update (only changing 1–5 random fields). Verify all other fields remain identical to their pre-update values.

5. **Rational threshold consistency** — For all governance threshold rationals in ProtocolParameters, verify: denominator > 0, numerator >= 0, numerator <= denominator (threshold in [0, 1]).

6. **Monotonic protocol version** — Apply a protocol version update. Verify the new version is strictly greater than the old (major > old_major, or major == old_major && minor > old_minor). Downgrades are rejected.

## Implementation Notes

- Reuse existing strategies from `cbor_proptest.rs` and `proptest_roundtrips.rs` where applicable (arb_hash32, arb_hash28, arb_tx_input, arb_value)
- The `arb_ledger_state` generator must produce internally consistent state — treasury + reserves + utxo_total + rewards + deposits = max_lovelace (45 billion ADA)
- Epoch transition tests need `arb_ledger_state` positioned at an epoch boundary slot
- UTxO tests need transaction builders that produce valid txs against the generated UTxO set
- Mempool tests are lighter — transactions only need valid structure, not valid signatures
- Protocol param tests can use simpler state since they focus on parameter fields, not full ledger
- All tests use `tempfile::tempdir()` for any disk-backed state (UTxO-HD via dugite-lsm)
