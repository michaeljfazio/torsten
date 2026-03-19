---
name: Issue #186 Plutus validation test coverage
description: Tests added for is_valid=false UTxO behaviour, treasury Phase-1 check, and the redeemer_script_version_map function that was needed to fix the build.
type: project
---

## Tests added for issue #186

### state/tests.rs — `test_issue_186_invalid_tx_utxo_exact_state`
Comprehensive single test asserting all four `is_valid=false` UTxO properties:
1. Collateral input consumed (absent after apply)
2. Regular input NOT consumed (present after apply)
3. Regular output NOT created (absent after apply)
4. Collateral return output created at index `tx.body.outputs.len()` with correct value

Helper `make_invalid_tx_with_col_return(tx_hash_byte, regular_input, collateral_input)` creates a Transaction with `is_valid=false` that has one regular in/out, one collateral input (10 ADA), and collateral_return (7 ADA).

### validation/tests.rs — treasury Phase-1 tests
Three tests for `validate_transaction_with_pools` treasury check (Conway, protocol_version_major >= 9):
- `test_issue_186_treasury_value_mismatch_rejects`: declared=999, actual=500 → `TreasuryValueMismatch`
- `test_issue_186_treasury_value_match_passes`: declared=500, actual=500 → no mismatch error
- `test_issue_186_treasury_value_none_skips_check`: `current_treasury=None` → check skipped entirely

### Per-tx ref script size (already existed)
The state/tests.rs file already had three `test_issue_184_*` tests for the 200 KiB per-tx limit. No duplicates were added.

## Also fixed: redeemer_script_version_map (build was broken in working tree)

The working tree had `plutus.rs` importing `redeemer_script_version_map` from `crate::validation` and `mod.rs` re-exporting it, but the function body in `collateral.rs` was missing. The function was implemented as part of this issue:

```
pub(crate) fn redeemer_script_version_map(
    tx: &Transaction,
    utxo_set: &UtxoSet,
    version_map: &HashMap<Hash28, u8>,
) -> HashMap<(u8, u32), u8>
```

Maps `(tag_byte, redeemer_index)` → language version (1/2/3) for each redeemer.
Tag byte encoding: 0=Spend, 1=Mint, 2=Cert, 3=Reward, 4=Vote, 5=Propose.
Used in `evaluate_plutus_scripts` so the V3 Unit-return check is applied per-redeemer
instead of to all redeemers when any V3 script is present in a mixed-version tx.

**Why:** Per-redeemer V3 Unit check is correct; the prior `has_any_v3` transaction-wide
flag incorrectly rejected V1/V2 scripts returning non-Unit in mixed-version transactions.

**How to apply:** When editing plutus.rs or collateral.rs, ensure this map is built once
per transaction call, not cached across transactions.
