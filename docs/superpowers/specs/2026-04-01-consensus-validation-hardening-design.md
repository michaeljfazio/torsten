# Consensus Validation Hardening

**Issue:** #323
**Date:** 2026-04-01
**Status:** Approved

## Summary

Harden consensus validation to match the Haskell cardano-node exactly. Four changes: replace hardcoded `MAX_SUPPORTED_PROTOCOL_MAJOR` with a config-driven `ObsoleteNode` check against the ledger's current protocol version, add a `HeaderProtVerTooHigh` upper-bound check on block header protocol versions, align chain selection tiebreaking with Haskell (remove hash fallback, add Conway slot-window restriction), and add IOHK Sum6Kes reference test vectors.

Three items from the original issue (#3 block size, #5 KES binding, #7 Shelley transition) were found to already be correct after cross-validation with the Haskell implementation.

## Changes

### 1. ObsoleteNode Check (replaces hardcoded MAX_SUPPORTED_PROTOCOL_MAJOR)

**Current state:** `const MAX_SUPPORTED_PROTOCOL_MAJOR: u16 = 10` in `praos.rs`, compared against the block header's protocol version.

**Haskell behavior:** `MaxMajorProtVer` is a static node config value (currently 10, or 11 with `ExperimentalHardForksEnabled`). The `envelopeChecks` function compares the **ledger's current protocol version** (`ppProtocolVersion.major` from the ticked `LedgerView`) against this value. If `ledger_pv.major > max_major_pv`, the node emits `ObsoleteNode` and rejects the block. This means the node's software is too old to follow the chain after a hard fork.

**Source:** `ouroboros-consensus-cardano/src/shelley/.../Protocol/Praos.hs` — `envelopeChecks`; `cardano-node/src/Cardano/Node/Protocol/Cardano.hs` — `cardanoProtocolVersion`.

**Changes:**

1. Add `max_major_prot_ver: u16` field to `PraosValidator` (or equivalent consensus config), defaulting to `10`.
2. In `validate_header` and `validate_header_full`, replace the current check:
   - **Before:** `if block.header.protocol_version.major > MAX_SUPPORTED_PROTOCOL_MAJOR` → `UnsupportedProtocolVersion`
   - **After:** `if protocol_params.protocol_version.major > self.max_major_prot_ver` → `ObsoleteNode`
3. Rename the error variant from `UnsupportedProtocolVersion` to `ObsoleteNode { chain_pv: u16, node_max_pv: u16 }` for clarity.
4. The `GetMaxMajorProtVersion` N2C query (tag 38, V21+) should return this static config value.

### 2. HeaderProtVerTooHigh Check

**Current state:** No check on block header protocol version relative to the ledger's current version.

**Haskell behavior:** The Conway BBODY rule checks `block_header_pv.major <= current_ledger_pv.major + 1`. If the block header claims a protocol version more than one major version ahead of the current ledger, it is rejected with `HeaderProtVerTooHigh`. This is an upper-bound-only check — there is no lower-bound "minimum PV for this era" check (the HFC telescope prevents era mismatches structurally via Haskell's type system).

**Source:** `cardano-ledger/eras/conway/impl/src/Cardano/Ledger/Conway/Rules/Bbody.hs` — `checkHeaderProtVerTooHigh`.

**Changes:**

1. Add `ConsensusError::HeaderProtVerTooHigh { supplied: u16, max_expected: u16 }` error variant.
2. In `validate_header` / `validate_header_full`, after the `ObsoleteNode` check, add:
   ```rust
   let current_pv_major = protocol_params.protocol_version.major;
   if let Some(next_pv) = current_pv_major.checked_add(1) {
       if block_header_pv_major > next_pv {
           return Err(ConsensusError::HeaderProtVerTooHigh {
               supplied: block_header_pv_major,
               max_expected: next_pv,
           });
       }
   }
   ```
3. This check applies to all post-Byron blocks (Shelley onwards), matching Conway BBODY behavior.

### 3. Chain Selection: Remove Hash Tiebreaker, Add Conway Slot Window

**Current state:** `prefer_chain()` in `chain_selection.rs` uses a hash-based fallback for Byron blocks. `praos_tiebreak()` compares VRF values unconditionally regardless of era.

**Haskell behavior:** No hash-based tiebreaking anywhere. The `comparePraos` function in `Praos/Common.hs` uses:
- Block number first (longer chain wins)
- Same slot + same issuer → higher opcert counter wins
- VRF tiebreaker: lower raw VRF output wins (lexicographic byte comparison)
- **Conway** (`RestrictedVRFTiebreaker 5`): VRF comparison only when `|slot_a - slot_b| <= 5`
- **Pre-Conway** (`UnrestrictedVRFTiebreaker`): always compare VRF
- On complete tie → no switch (incumbent wins)

**Source:** `ouroboros-consensus-protocol/src/.../Protocol/Praos/Common.hs` — `comparePraos`.

**Changes:**

1. Remove hash-based fallback from `prefer_chain()`. On tie, return no preference (incumbent wins).
2. Add `era` parameter to `praos_tiebreak()` (or derive from block metadata).
3. For Conway (era >= 9): only run VRF comparison when `|slot_a - slot_b| <= 5`. Otherwise return no preference.
4. For pre-Conway: keep unrestricted VRF comparison (current behavior).
5. Verify `vrf_tiebreak()` uses lexicographic comparison with lower value winning (should already be correct).

### 4. Sum6Kes IOHK Reference Test Vectors

**Current state:** No IOHK reference test vectors for Sum6Kes in the test suite.

**Changes:**

1. Add a test module in `crates/dugite-crypto/src/kes.rs` (or `tests/`) with IOHK reference vectors.
2. Test cases: known seed → known keypair → known signature at known period → verify passes.
3. Also test: verification with wrong key fails, verification at wrong period fails, verification with corrupted signature fails.

## Items Confirmed Correct (No Changes)

### Block Size Validation (Issue Item #3)
Haskell checks `header_size <= maxBlockHeaderSize` and `body_size <= maxBlockBodySize` separately in `envelopeChecks`. Our `validate_envelope()` already does exactly this. The issue's suggestion of `body + header <= total` does not match Haskell.

### KES Explicit Binding (Issue Item #5)
The binding between `opcert.kes_vk_hot` and the actual KES signing key is enforced implicitly by `KES.verifySignedKES` — it takes `vk_hot` from the opcert as the verification key and the Sum6Kes Merkle root reconstruction internally validates the key match. There is no explicit `vk_hot == derived_pk` comparison in Haskell. As long as Dugite passes `opcert.kes_vk_hot` to `kes_verify`, this is correct.

### Shelley Transition (Issue Item #7)
The Shelley transition epoch is not derivable from genesis config alone — it requires either on-chain Byron update proposals (mainnet) or `TestShelleyHardForkAtEpoch` config override (testnets). Dugite's `shelley_transition_epoch_for_magic()` lookup table + dynamic EraHistory detection already matches this correctly.

## Tests

1. **ObsoleteNode:** Set `max_major_prot_ver = 10`, create ledger state with `protocol_version.major = 11` → verify block rejected with `ObsoleteNode`
2. **HeaderProtVerTooHigh:** Current ledger PV is 9 (Conway), block header claims PV 11 → rejected; PV 10 → accepted
3. **Chain selection no-hash:** Two chains with same block number, different hashes → no preference (incumbent wins)
4. **Conway slot window:** Two Conway blocks with same block number, different VRF, slots 10 apart → no preference; slots 3 apart → lower VRF wins
5. **Pre-Conway unrestricted VRF:** Two Babbage blocks with same block number, different VRF, slots 100 apart → lower VRF still wins
6. **Sum6Kes IOHK vectors:** Known key/signature/period → verification passes; wrong inputs → verification fails

## Files Modified

- `crates/dugite-consensus/src/praos.rs` — ObsoleteNode check, HeaderProtVerTooHigh check
- `crates/dugite-consensus/src/chain_selection.rs` — remove hash tiebreaker, add Conway slot window
- `crates/dugite-crypto/src/kes.rs` — Sum6Kes test vectors
