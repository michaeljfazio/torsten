---
name: Forge Pipeline Audit 2026-03-23
description: Block forging pipeline audit findings — nonce double-hashing, set-snapshot for leader, header body encoding
type: project
---

## Critical Issue: nonce_vrf_output double-hashing in forge.rs

**Where:** `crates/dugite-node/src/forge.rs` lines 291-296
**Problem:** When forging, we compute:
  `nonce_vrf_output = blake2b_256("N" || vrf_output)`    [1 hash, 32 bytes]

But `update_evolving_nonce()` in `epoch.rs` line 721 ALWAYS hashes its input again:
  `eta_hash = blake2b_256(nonce_vrf_output)`
  `evolving' = blake2b_256(evolving || eta_hash)`

So when our forged block is applied back to OUR OWN ledger, the nonce is updated with a DOUBLE-hashed value:
  `blake2b_256(blake2b_256("N" || vrf_output))`

But for INCOMING blocks decoded by pallas, `multi_era.rs` line 217 calls `hb.nonce_vrf_output()` which returns `blake2b_256("N"||vrf_result.0)` — one hash — and then `update_evolving_nonce` adds the second hash. This path is correct for received blocks.

For our forged blocks we pre-compute one hash and store it as `nonce_vrf_output`. When applied, it gets hashed a second time by `update_evolving_nonce`, resulting in a 3-hash chain instead of the correct 2-hash chain.

**Impact:** After forging a block, our local epoch nonce diverges from the canonical chain's nonce. This causes VRF verification failures for subsequent slots, and our next block's epoch nonce field will be wrong.

**Fix:** In forge.rs, store the RAW 64-byte vrf_output in nonce_vrf_output (matching the TPraos path), OR don't hash it at all and let update_evolving_nonce handle the full computation. The cleanest fix: store the raw vrf_output directly. Then update_evolving_nonce will compute blake2b_256(raw) as eta_hash then blake2b_256(evolving||eta_hash). BUT wait — for TPraos (Shelley-Alonzo), multi_era.rs stores the raw 64-byte nonce_vrf.0 and update_evolving_nonce hashes it once. For Praos (Babbage/Conway), multi_era.rs stores blake2b_256("N"||vrf_result.0) = 32 bytes, and update_evolving_nonce hashes THAT again. So for Praos the total is: blake2b_256(blake2b_256("N"||vrf)) — 2 hashes on the tagged output. Our forge stores blake2b_256("N"||vrf) (1 hash), then update_evolving_nonce adds 1 more = correct! So actually the forge path may be CORRECT for nonce update. Need to verify against test vectors.

**Why:** The design was established when the nonce double-hash was discovered for incoming blocks. The forge path was written to match but the behavior needs verification with actual test vectors.

## Leadership Check Uses Correct "set" Snapshot

**Confirmed correct:** `try_forge_block_at` in `node/mod.rs` lines 2571-2578 uses `ls.snapshots.set` for pool stake. This is the Haskell spec (epoch N-1 snapshot drives epoch N+1 leadership). Correct.

## VRF Input Construction

**Confirmed correct:** `vrf_input()` in `slot_leader.rs` line 50-55: `blake2b_256(slot_BE_8bytes || epoch_nonce_32bytes)`. Matches Haskell.

## Header Body CBOR Encoding

**Confirmed correct:** `encode_block_header_body` in `encode/block.rs` lines 38-51: 10-element array matching pallas Babbage HeaderBody fields 0-9. Field order: block_number, slot, prev_hash, issuer_vkey, vrf_vkey, vrf_result, body_size, body_hash, opcert, protocol_version. Matches the pallas model at model.rs fields #n(0)-#n(9).

## epoch_nonce Field in BlockHeader

**Non-issue:** The `epoch_nonce` field in `BlockHeader` is NOT encoded into the CBOR header body (it's absent from `encode_block_header_body`). It is only stored in the Dugite in-memory struct for convenience. The epoch_nonce stored in `forge.rs` line 310 does NOT appear on the wire. Correct.

## VRF Leader Value Derivation (Praos, proto>=7)

**Confirmed correct:** `vrf_leader_value()` in `slot_leader.rs` line 60-65: `blake2b_256("L" || vrf_output)`. Matches pallas `derive_tagged_vrf_output(vrf_result.0, Leader)`.

## KES Period Calculation

**Confirmed correct:** `forge.rs` line 329: `current_slot / slots_per_kes_period`. Offset = `current_kes_period - opcert_kes_period`. Signs via `kes_evolve_to_period(kes_skey, offset)`. This is the correct KES period for the signature.

## Opcert Signable Bytes

**Confirmed correct:** `praos.rs` lines 1122-1127: `hot_vkey(32) || sequence_number(8 BE) || kes_period(8 BE)`. Raw bytes, NOT CBOR. Matches Haskell OCertSignable.

## Block Announcement

**Confirmed correct:** `node/mod.rs` lines 2679-2741: ChainDB write FIRST, then ledger apply with ValidateAll, THEN broadcast via `block_announcement_tx`. Correct ordering.

## Stake Snapshot for Leader vs Haskell Spec

Haskell uses the "mark" snapshot (ssStakeMark) for leader election in epoch N+2, not "set". The "set" snapshot is used for reward calculation. Dugite uses "set" for leader check.

**This is a potential bug:** Haskell's `checkLeaderValue` in `Cardano.Protocol.TPraos.BHeader` uses the stake from the MARK snapshot (2 epochs old), while Dugite uses the SET snapshot (1 epoch old). Need to verify this against the spec. The Ouroboros Praos paper says the stake distribution used for leader election is fixed at the epoch boundary 2 epochs prior, which is the "mark" snapshot from the current epoch.

**Actually:** Per the Haskell implementation, the stake snapshot used for slot leader selection in epoch E is the "mark" snapshot taken at the START of epoch E-1 (i.e., it's the snapshot that was "mark" at the end of E-2 / beginning of E-1). In the mark/set/go cycle: mark becomes set at E-1 boundary, set becomes go at E boundary. The stake USED for leader election in epoch E is the "go" snapshot. In Dugite, the forge path uses "set" which corresponds to one epoch behind "go". This may cause leader election discrepancies.
