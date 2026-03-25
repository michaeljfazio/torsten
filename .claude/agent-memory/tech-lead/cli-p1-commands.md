---
name: P1 CLI commands — issue #190
description: Implementation of the six highest-priority cardano-cli parity commands from issue #190
type: project
---

Six P1 commands implemented (2026-03-19) from GitHub issue #190.

**Why:** These are the most commonly used offline tx-building and node-inspection commands; every user building transactions needs calculate-min-fee and calculate-min-required-utxo.

**Commands and status:**

| Command | Status | Notes |
|---|---|---|
| `transaction calculate-min-fee` | Extended | Added `--tx-in-count` / `--tx-out-count` compat flags (ignored; body size measured directly). Delegates to `estimate_fee()`. |
| `transaction calculate-min-required-utxo` | New | Uses `min_ada_for_output()` + `parse_tx_output()`. JSON output `{"lovelace": N}` matching cardano-cli. |
| `transaction policyid` | Already done | Script hash = blake2b_224(0x00 || native_script_cbor). |
| `query pool-params` | Already done | Human-readable output (not cardano-cli JSON yet). |
| `query slot-number` | New | Calls `query_era_history()` (QueryHardFork/GetInterpreter) + `query_system_start()`. Slot formula: `zero_slot + floor((target - era_start) * 1000 / slot_length_ms)`. |
| `query kes-period-info` | New | Calls `query_current_kes_period()` (Shelley tag 31). Decodes opcert CBOR array(2)[array(4)[hot_vkey, counter, kes_period, sig], cold_vkey]. Reports VALID/EXPIRED/NOT_YET_VALID. |

**N2C client additions in `n2c_client.rs`:**
- `query_era_history()` — sends `[3, [2, [0]]]` (QueryHardFork/GetInterpreter). Response is raw MsgResult, NOT HFC-wrapped.
- `query_current_kes_period()` — sends Shelley tag 31. Response IS HFC-wrapped.

**Era history CBOR wire format (from query_handler tests):**
- Indefinite array of EraSummary, each = `array(3)[EraParams, EraStart, SafeZone]`
- EraParams = `array(3)[epoch_length, slot_length_ms, safe_zone]`
- EraStart  = `array(2)[slot_offset, time_offset_ms]`  (time_offset relative to system start)
- NOT wrapped in HFC success array(1)

**How to apply:** When adding more slot-related queries, reuse `query_era_history()` from `n2c_client.rs`. The response format (indefinite array, no HFC wrapper) is different from all Shelley-era queries which use `send_query()` and strip the HFC wrapper.
