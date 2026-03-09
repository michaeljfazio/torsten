# VRF Verification Debugging Notes

## Session: 2026-03-09 — Investigating VRF fix in commit eb5306b

### Commit eb5306b changes
1. `vrf_input()` in `slot_leader.rs`: changed ordering and added Blake2b-256 hash
   - Old: `epoch_nonce(32B) || slot_u64_BE(8B)` — wrong order, not hashed
   - New: `Blake2b-256(slot_u64_BE(8B) || epoch_nonce(32B))` — slot first, then hash
2. `verify_vrf_proof()` in `praos.rs`: now uses `slot_leader::vrf_input()` for seed

### Why VRF STILL FAILS after eb5306b

**Diagnostic evidence** (from warn! logging added temporarily):
```
slot=106399569
vrf_vkey_hex="9bb043d5ae1b8c4c2ae57465466d82487c9b0f03a770ea038c34fd6c5c5afc07"
vrf_vkey_len=32
vrf_proof_len=80
vrf_output_len=64
seed_hex="e63f7a535edb56b9715a430d5de4b6b8bac47451de73dc7f80b55415da2e5e6f"
epoch_nonce_hex="0000000000000000000000000000000000000000000000000000000000000000"
```

The epoch_nonce is all zeros! This means:
- `header.epoch_nonce` is `Hash32::ZERO` because the block header wire format does NOT
  carry the epoch nonce — it must be injected from the current ledger state
- The validate_header call in `node.rs::process_forward_blocks()` was passing the header
  directly from the wire (which has zero epoch nonce)

### Fix applied to node.rs
In `process_forward_blocks()`, before calling `consensus.validate_header()`:
```rust
let epoch_nonce = {
    let ls = self.ledger_state.read().await;
    ls.epoch_nonce
};
let mut header_with_nonce = last_block.header.clone();
header_with_nonce.epoch_nonce = epoch_nonce;
// then validate_header(&header_with_nonce, ...)
```

### Why VRF STILL FAILS even after epoch nonce injection fix

Even with correct injection, the ledger `epoch_nonce` is wrong:
- Scenario A: ledger starts fresh (snapshot fails) → epoch_nonce = Hash32::ZERO
- Scenario B: snapshot loads but was saved during fresh-start → epoch_nonce = hash(genesis||genesis)
  = `73510a8b4803a83fad511f6b260def9e9814a8e0d93d904936c94f010f2c4c6c`

The correct epoch 1231 nonce on preview testnet is completely different — it's the result of
accumulating rolling nonces over all 4M+ blocks since genesis.

### Snapshot failure root cause
Error: `string is not valid utf8: invalid utf-8 sequence of 1 bytes from index 1565905`
This is a `bincode` deserialization error. Bincode is type-length-value encoding, and when a
`String` field is changed to `Vec<u8>` (or vice versa), the byte count prefix can be misread
as string content, causing UTF-8 decode failures.

To identify which field changed: compare `LedgerState` struct fields between the old snapshot
commit and current HEAD. The snapshot was saved before the "phantom UTxO inflation" fix (fd838c5).

### What VRF verification SHOULD look like when working

From `vrf_dalek::vrf03::VrfProof03::verify()`:
1. `hash_to_curve(public_key, alpha_string)` → H using elligator2 (IOG fork)
2. Check pk is not small-order
3. Compute U = response*G - challenge*PK
4. Compute V = response*H - challenge*Gamma
5. Recompute challenge from (H, Gamma, U, V)
6. Compare with stored challenge (first 16 bytes match = success)
7. Return `proof_to_hash()` = SHA512(SUITE || 0x03 || Gamma_cofactor)

The alpha_string is the VRF seed = Blake2b-256(slot_u64_BE || epoch_nonce).

### Key insight: epoch_nonce is NOT in the block header wire format
Cardano's block header CBOR does not include the epoch nonce. The epoch nonce is a ledger-level
concept, computed from:
1. The rolling nonce (eta_v) accumulated over the nonce contribution window (first 3k/f slots)
2. The hash of the first block of the previous epoch (nh)
3. `epoch_nonce = Blake2b-256(eta_v || nh)`

The `BlockHeader.epoch_nonce` field in our Rust struct is always set to `Hash32::ZERO` during
deserialization in `multi_era.rs` (line 1810 of state.rs context). It MUST be injected from
the ledger state before any VRF computation.

### Required fix pipeline
1. Fix ledger snapshot backward compat (bincode migration for struct field change)
2. OR: Re-import Mithril snapshot to rebuild ledger state with correct epoch nonce
3. The structural code fixes (vrf_input ordering + epoch nonce injection) are in place

### vrf_dalek library structure
- `vrf03`: IETF draft-03, uses curve25519-dalek-FORK (IOG elligator2), Cardano's algorithm
- `vrf10`: IETF draft-10, uses standard curve25519-dalek, different hash-to-curve
- `vrf10_batchcompat`: IETF draft-10 batch verification variant
- Cardano uses `vrf03` (the IOG VRF03 / libsodium-based algorithm)
- Test vectors for vrf03 match IOG's libsodium test vectors exactly
