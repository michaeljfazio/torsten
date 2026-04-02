---
name: output_cbor_reencode
description: TransactionOutput CBOR re-encoding bugs found and fixed (2026-03-15)
type: project
---

## Two Bugs Fixed in TransactionOutput Re-encoding

### Context
When `TransactionOutput` is stored in the LSM store (via bincode), `raw_cbor: Option<Vec<u8>>`
is dropped (`#[serde(skip)]`). On read-back, `encode_transaction_output()` must reproduce
the exact original CBOR bytes.

**Why exact bytes matter:**
- `uplc::tx::eval_phase_two_raw()` takes UTxO output CBOR verbatim for script context
- Plutus scripts that hash the context get wrong results if bytes differ
- N2C `GetUTxO` responses must match cardano-node exactly

---

### Bug 1: Indefinite-length Inline Datum Encoding

**Root cause:** `encode_plutus_data()` always produces definite-length CBOR arrays (e.g. `0x85`
for array-of-5). Many Plutus script builders emit indefinite-length arrays (`0x9f...0xff`).
After LSM round-trip, the re-encoded datum differed.

**Specific example:** Inline datum `[0,0,0,0,0]`:
- Original on-chain: `9f 00 00 00 00 00 ff` (indef, 7 bytes)
- Fresh encode: `85 00 00 00 00 00` (definite, 6 bytes)
- This also changes the tag(24) length prefix: `d818 47` vs `d818 46`

**Fix:** Added `raw_cbor: Option<Vec<u8>>` field to `OutputDatum::InlineDatum`:
```rust
InlineDatum {
    data: PlutusData,
    raw_cbor: Option<Vec<u8>>,  // NOT serde(skip) — survives bincode
}
```
During deserialization from pallas, `d.0.raw_cbor().to_vec()` captures the original bytes.
`encode_transaction_output()` uses `raw_cbor` when present, falls back to `encode_plutus_data()`.

---

### Bug 2: Legacy vs Post-Alonzo Output Format

**Root cause:** Conway-era transactions can contain "legacy" outputs in Shelley-era array
format: `[address, value]` or `[address, value, datum_hash]`. Our encoder always used the
post-Alonzo map format `{0: addr, 1: value, ...}`. The first byte differs:
- Legacy: `0x82` (array(2))
- Post-Alonzo: `0xa2` (map(2))

**Fix:** Added `is_legacy: bool` to `TransactionOutput`:
```rust
pub struct TransactionOutput {
    // ...
    #[serde(default)]
    pub is_legacy: bool,  // NOT serde(skip) — survives bincode
    // ...
}
```
Set from `is_babbage_legacy()` / `is_conway_legacy()` during pallas deserialization by
matching `PseudoTransactionOutput::Legacy(_)` variants. `encode_transaction_output()`
dispatches to `encode_legacy_transaction_output()` when `is_legacy` is true.

---

### Key Invariants
- Legacy outputs NEVER have `InlineDatum` (only `DatumHash` or `None`) — no raw datum needed
- Indefinite-length arrays only appear inside tag(24)-wrapped inline datums, NOT in the outer output map
- `encode_legacy_transaction_output()`: `[addr_bytes, value]` or `[addr_bytes, value, datum_hash]`
- pallas: `MintedDatumOption = PseudoDatumOption<KeepRaw<PlutusData>>` — raw bytes via `.raw_cbor()`

---

### Test Coverage Added
`crates/dugite-serialization/tests/output_reencode.rs` — 6 tests:
1. `test_output_reencode_simple_indef_datum` — tx with indef-length 5-element list datum
2. `test_output_reencode_nested_constr_datum` — deeply-nested Constr all using indef arrays
3. `test_output_reencode_multi_output_mixed_datums` — 4-output tx with multiple datum shapes
4. `test_inline_datum_raw_cbor_populated` — verifies raw bytes captured at deserialization
5. `test_inline_datum_raw_cbor_survives_bincode` — verifies raw bytes NOT dropped by bincode
6. `test_indef_array_encoding_difference_documented` — documents the encoding divergence

All 3 real tx vectors fetched from Preview testnet (epoch 1237) via Koios on 2026-03-15.

**Why:** Stale `raw_cbor` on `TransactionOutput` was already `#[serde(skip)]` and that's
fine for the outer output bytes — what matters is that the inner datum bytes survive.
