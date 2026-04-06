# Proptest Expansion Design — Issue #342

**Date:** 2026-04-06 (revised after Haskell cross-validation)
**Issue:** #342 — Expand proptest coverage for epoch transitions, UTxO invariants, and mempool

## Overview

Add 30 new property-based tests across 4 domains: epoch transitions (7), UTxO invariants (10), mempool invariants (7), and protocol parameter transitions (6). Full state machine testing approach — generate complete LedgerState instances with pools, delegations, UTxO sets, and governance state, then verify invariants through transitions.

All properties have been cross-validated against the Haskell cardano-ledger and cardano-node implementations.

## File Organization

| File | Crate | Cases | Properties |
|------|-------|-------|------------|
| `tests/strategies.rs` | dugite-ledger | — | Shared generators |
| `tests/epoch_proptest.rs` | dugite-ledger | 256 | 7 |
| `tests/utxo_proptest.rs` | dugite-ledger | 256 | 10 |
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
- `arb_valid_tx(utxo_set)` → Transaction whose inputs exist in the given UTxO set, with correct fee/value balance
- `arb_valid_block(utxo_set, n_txs)` → Block containing n valid transactions, handling intra-block chaining

### State Generators

- `arb_ledger_state(config)` → Full LedgerState with:
  - 1–20 registered pools with random params
  - 1–100 delegations across those pools
  - 100–1000 UTxO entries
  - Treasury + reserves + utxo_total + reward_accounts + deposits_pot + fee_pot == max_lovelace (45B ADA)
  - 0–10 DRep registrations
  - Valid protocol parameters
  - Consistent snapshot state (mark/set/go)
  - deposits_pot == totalObligation(certState, govState)

The `config` parameter controls bounds (pool count range, delegation count, UTxO count) so each test domain can tune generation to its needs.

## Epoch Transition Tests

`dugite-ledger/tests/epoch_proptest.rs` — 256 cases per property.

### Properties

1. **Reward distribution bounded by available pot** — Generate ledger state at epoch boundary, run `process_epoch_transition()`, verify `sum(all_rewards) <= deltaR1 + fees - floor(tau * (deltaR1 + fees))`. The total distributed rewards never exceed the available reward pot after treasury cut. (Note: individual rewards are trivially non-negative since `Coin` is unsigned.)

2. **Total ADA conservation** — Before and after transition, the six-pot identity holds: `utxo_total + reserves + treasury + sum(reward_accounts) + deposits_pot + fee_pot == max_lovelace_supply` (45,000,000,000,000,000 lovelace). All six terms must be accounted for; omitting deposits_pot or fee_pot will produce false failures.

3. **Snapshot rotation correctness** — At epoch N, verify: previous `mark` becomes `set`, previous `set` becomes `go`, and a new `mark` is computed from current stake distribution. The new `mark` is computed *after* rewards have been credited to accounts (so newly credited rewards are included in the stake distribution). Compare snapshots structurally.

4. **Pool retirement processing** — Register pools with `retiring_epoch <= current_epoch`, run transition, verify: (a) retired pools removed from `pool_params`, (b) pool deposit refunded to the pool's `reward_account` (or to treasury if that account is unregistered), (c) delegators are NOT undelegated — they become orphaned (still point at the dead pool ID, their stake becomes inactive at the next snapshot), (d) `future_pool_params` entries for retiring pools are also removed.

5. **Reward distribution formula** — For a single pool with multiple delegators: leader receives `max(pool_cost, poolReward * (margin + (1 - margin) * leader_stake / pool_stake))`. Remaining `poolReward - leader_reward` is split proportionally among members by stake share, with floor division per member. Total rounding loss is at most `n - 1` lovelace where `n` is the number of pool members. Pools with `poolReward <= pool_cost` pay only the leader; members receive zero.

6. **Protocol parameter update activation at N+1** — Enqueue a parameter update proposal during epoch N. Run transition to epoch N — verify old params still active. Run transition to epoch N+1 — verify new params are now active. The update takes effect one epoch after the proposal epoch, not at the proposal epoch boundary. (Conway: governance ratification at end of epoch N → active from epoch N+1.)

7. **Idempotent epoch detection** — Calling `process_epoch_transition()` with the same epoch number twice doesn't double-apply rewards or rotate snapshots twice. The guard is `current_epoch == succ(last_epoch)` — only the exact successor triggers the transition. Same-epoch calls may still advance DRep pulsing state but do not re-run the EPOCH pipeline.

## UTxO Invariant Tests

`dugite-ledger/tests/utxo_proptest.rs` — 256 cases per property.

### Properties

1. **Per-transaction ADA conservation** — For each valid transaction: `sum(consumed_input_values) + sum(withdrawals) + sum(deposit_refunds) == sum(output_values) + fee + sum(deposits_paid) + treasury_donation`. Minting does NOT appear in the ADA conservation equation — it only affects multi-asset balances. Verify per-transaction (not aggregated over the block) since each tx is individually balanced.

2. **Multi-asset conservation** — For each valid transaction with minting: `sum(inputs[policy][name]) + mint[policy][name] == sum(outputs[policy][name])` for every (policy, name) pair. This is orthogonal to ADA conservation.

3. **Minimum UTxO value enforcement** — After applying valid blocks, every UTxO entry satisfies `coin >= coinsPerUTxOByte * serialized_size(output)`. This is stronger than just `coin > 0`. Edge cases: Byron-era genesis UTxOs (translated at HFC, may have different rules) and `collateralReturn` outputs from `is_valid = false` transactions must also satisfy minUTxO.

4. **Rollback restores exact UTxO state** — Generate UTxO set, snapshot it, apply a valid block, then rollback via DiffSeq. Verify the UTxO set is byte-identical to the snapshot. Test with both single blocks and chains of 1–5 blocks. Note: this tests UTxO restoration only — deposits, certs, and governance state are separate rollback concerns not covered by DiffSeq.

5. **DiffSeq flush behavior** — DiffSeq has no automatic capacity limit; eviction is caller-driven via `flush_up_to(slot)`. After calling `flush_up_to(slot_N)`, verify: (a) `len()` equals the count of diffs with `slot > slot_N`, (b) all diffs at or below `slot_N` are removed, (c) remaining diffs are unmodified.

6. **DiffSeq rollback consistency** — Apply N blocks (N <= k), rollback M blocks (M <= N). Verify at the `UtxoStore` level (not just DiffSeq metadata) that the UTxO set matches the state after applying only the first N-M blocks. Include a case where Tx2 spends an output created by Tx1 in an earlier block — rollback must correctly handle the chain.

7. **Input consumption is atomic** — After a valid block with multiple transactions, every consumed input is absent from the UTxO set, and every produced output is present. No partial application. Include intra-block chained spending: Tx2 in the same block spends an output of Tx1, both are applied, and Tx1's consumed inputs are absent while Tx2's outputs are present.

8. **Duplicate input rejection** — Generate a transaction that references the same input twice. Verify validation rejects it before it touches the UTxO set. (Haskell catches this at deserialization in Conway via `OSet`; Dugite catches it in Phase-1 — same behavioral result.)

9. **Deposit pot invariant** — After every block application: `deposits_pot == totalObligation(certState, govState)` where `totalObligation = sum(keyDeposit per registered credential) + sum(poolDeposit per registered pool) + sum(dRepDeposit per registered DRep) + sum(govActionDeposit per active proposal)`. This is critical for catching deposit tracking bugs (known HIGH-priority gap).

10. **Collateral UTxO invariant** — For transactions with `is_valid = false`: only collateral inputs are consumed (removed from UTxO), `collateral_return` output is added to UTxO, and spending inputs remain in the UTxO untouched. Regular outputs from the transaction body are NOT added.

## Mempool Invariant Tests

`dugite-mempool/tests/mempool_proptest.rs` — 1000 cases per property.

### Properties

1. **No duplicate transaction IDs** — Add 10–50 random transactions. After each add, verify all tx IDs in the mempool are unique. (Uniqueness is enforced via `AlreadyExists` check and input-conflict detection.)

2. **Five-dimensional capacity enforcement** — Configure mempool with specific limits. After each add, verify ALL five dimensions are respected: (a) `len() <= max_transactions`, (b) `total_bytes() <= max_bytes`, (c) `total_ex_mem() <= max_ex_mem`, (d) `total_ex_steps() <= max_ex_steps`, (e) `total_ref_scripts_bytes() <= max_ref_scripts_bytes`. When capacity is exceeded, eviction removes the lowest-fee-density transaction; new tx is rejected with `InsufficientPriority` if it has lower fee density than the worst existing tx.

3. **TTL sweep completeness** — Add transactions with random TTL values (slot-based, half-open interval: valid while `current_slot < ttl`). Call `evict_expired(current_slot)`. Verify: no transaction with `ttl <= current_slot` remains, all transactions with `ttl > current_slot` or `ttl = None` are still present.

4. **Input conflict detection** — Generate two transactions that spend the same input (spending inputs only — reference inputs and collateral are shareable). Add the first (succeeds). Add the second — verify it's rejected with `InputConflict`. The first transaction remains unaffected.

5. **Removal frees inputs and cascades dependents** — Add a parent transaction Tx_A. Add a child Tx_B that spends an output of Tx_A (via virtual UTxO). Remove Tx_A. Verify: (a) Tx_A's spending inputs are freed (a new tx can claim them), (b) Tx_B is cascade-removed (BFS on dependency graph), (c) Tx_B's inputs are also freed.

6. **FIFO block production ordering** — Add transactions in a known order. Call `get_txs_for_block()`. Verify the returned transactions are in insertion order (oldest first), taking the longest FIFO prefix that fits within block capacity. This matches the Haskell node's block production strategy. Fee density is used ONLY for eviction decisions, not for block selection or iteration.

7. **Dual-FIFO fairness** — Add a mix of Local and Remote origin transactions. Verify both origins are represented in the mempool (neither origin starves the other when capacity allows). Local submissions lock 1 mutex (`all_fifo`), remote lock 2 (`remote_fifo` then `all_fifo`), giving each local client equal weight to all remote peers combined.

## Protocol Parameter Transition Tests

`dugite-ledger/tests/protocol_params_proptest.rs` — 1000 cases per property.

### Properties

1. **CBOR-enforced parameter bounds** — Generate random ProtocolParameters. The only hard bounds enforced by the ledger are those from CBOR encoding: all `uint` fields are non-negative (including `min_fee_a` and `min_fee_b`, which CAN be 0), all `positive_uint` denominators are >= 1. Governance thresholds are NOT constrained to [0, 1] by the ledger — a threshold > 1.0 is valid CBOR and simply unmeetable. Additional bounds may be enforced by the on-chain guardrail script (Conway), but that is script-defined, not ledger-enforced. Verify: generated params with valid CBOR types are accepted; params violating CBOR type constraints are rejected.

2. **Update mechanism per era** — Two completely separate systems:
   - **Pre-Conway (Shelley–Babbage):** `ProposedPPUpdates` using genesis delegate keys. All submitting delegates must agree on the same update (unanimity, not a fraction). One update per epoch. Accepted when epoch boundary arrives and the update epoch matches.
   - **Conway:** `ParameterChange` governance action. Ratified when DRep vote ratio >= `dvtPPGroupThreshold` (varies by PP group: Network, Economic, Technical, Gov) AND SPO security vote passes (for SecurityGroup params) AND CC vote passes. No genesis delegates involved. Test both mechanisms separately.

3. **Era-specific parameter presence** — For each era (Shelley through Conway), generate params and verify era-specific fields: Shelley has no `collateral_percentage`; Alonzo adds `collateral_percentage`, `prices`, `maxTxExUnits`, `maxBlockExUnits`, `maxValSize`, `maxCollateralInputs`, `costModels`, `coinsPerUTxOByte`; Babbage removes `minUTxOValue`, `ppD` (decentralization), `extraEntropy`; Conway adds `poolVotingThresholds`, `drepVotingThresholds`, `committeeMinSize`, `committeeMaxTermLength`, `govActionLifetime`, `govActionDeposit`, `drepDeposit`, `drepActivity`, `minFeeRefScriptCostPerByte`. Conway's `protocolVersion` field is present (array index 12) but tagged `HKDNoUpdate` — it cannot be changed via `ParameterChange`, only via `HardForkInitiation`.

4. **Update preserves unchanged fields** — Apply a partial parameter update (only changing 1–5 random fields via `Some`/`SJust`). Verify all `None`/`SNothing` fields remain identical to their pre-update values. This matches the Haskell `updatePP` function's identity behavior on `SNothing` fields.

5. **Rational threshold CBOR validity** — For all governance threshold rationals in ProtocolParameters (encoded as `Tag(30)[numerator, denominator]`): verify `denominator >= 1` (CBOR `positive_uint`) and `numerator >= 0` (CBOR `uint`). Note: `numerator <= denominator` is NOT enforced by the Haskell ledger — thresholds > 1.0 are valid on-chain (just permanently unmeetable). Do not assert `ratio <= 1.0`.

6. **Monotonic protocol version (lexicographic)** — Apply a protocol version update via `HardForkInitiation`. Verify the new version is strictly greater than the old using lexicographic pair comparison: `(major', minor') > (major, minor)` iff `major' > major` OR (`major' == major` AND `minor' > minor`). Minor CAN decrease when major increases (e.g., `(9, 0) → (10, 0)` is valid). Downgrades are rejected with `ProposalCantFollow`.

## Implementation Notes

- Reuse existing strategies from `cbor_proptest.rs` and `proptest_roundtrips.rs` where applicable (arb_hash32, arb_hash28, arb_tx_input, arb_value)
- The `arb_ledger_state` generator must produce internally consistent state with the six-pot identity: `treasury + reserves + utxo_total + reward_accounts + deposits_pot + fee_pot == max_lovelace`
- The `arb_ledger_state` generator must also ensure `deposits_pot == totalObligation(certState, govState)`
- Epoch transition tests need `arb_ledger_state` positioned at an epoch boundary slot
- UTxO tests need transaction builders that produce valid txs against the generated UTxO set, including intra-block chaining
- Mempool tests are lighter — transactions only need valid structure, not valid signatures; use existing `make_dummy_tx()` and related test helpers
- Protocol param tests can use simpler state since they focus on parameter fields, not full ledger
- All tests use `tempfile::tempdir()` for any disk-backed state (UTxO-HD via dugite-lsm)

## Haskell Cross-Validation Notes

The following corrections were applied after cross-validation against cardano-ledger and cardano-node:

- **Rewards non-negative** → replaced with reward pot bound (Coin is unsigned, making non-negativity trivially true)
- **ADA conservation** → expanded to six-pot identity including deposits_pot and fee_pot
- **Pool retirement** → delegators are NOT undelegated; deposit refunded to pool's reward account only
- **Reward proportionality** → corrected for leader cost/margin deduction and O(n) rounding tolerance
- **PParam activation** → corrected to N+1 (not N)
- **UTxO conservation** → corrected to per-tx formula with withdrawals, deposits, refunds, donation; minting removed from ADA balance
- **No zero-value UTxO** → strengthened to full minUTxOValue formula
- **DiffSeq capacity** → rewritten as flush_up_to behavior (no automatic capacity limit)
- **Fee density ordering** → replaced with FIFO ordering (fee density is eviction-only)
- **Mempool size limits** → expanded to five-dimensional capacity (count, bytes, ex_mem, ex_steps, ref_scripts)
- **Parameter bounds** → weakened to match actual ledger enforcement (min_fee_a=0 is legal, thresholds > 1.0 are valid)
- **Update quorum** → split into pre-Conway unanimity vs Conway governance vote ratios
- **Rational thresholds** → removed `numerator <= denominator` constraint (not enforced by ledger)
- **Protocol version monotonicity** → clarified as lexicographic pair (minor can decrease when major increases)
- **Added 3 new UTxO properties**: deposit pot invariant, collateral UTxO invariant, multi-asset conservation
