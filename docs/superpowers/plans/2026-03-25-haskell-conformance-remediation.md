# Haskell Conformance Remediation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring Torsten into full protocol conformance with the Haskell cardano-node across serialization, networking, consensus, ledger validation, and storage.

**Architecture:** This plan addresses 45+ issues found in a deep comparative review, organized into 6 phases by severity. Each phase produces independently testable, committable work. Phases are ordered so that earlier phases unblock later ones (e.g., fixing wire encoding before fixing validation that depends on correct encoding).

**Tech Stack:** Rust, minicbor (CBOR), pallas crates, tokio, blake2b

**References:**
- Conway CDDL: `eras/conway/impl/cddl/data/conway.cddl` in cardano-ledger repo
- Haskell ouroboros-network for protocol codecs
- Haskell cardano-ledger for STS rules
- Review findings: conversation context (2026-03-25 deep review)

**Notes:**
- Line numbers are approximate — earlier tasks may shift them. Search for the relevant patterns/function names.
- Two issues from the original review were verified as non-issues during plan review:
  - ~~C2 (protocol param update keys)~~: Verified correct — keys 0-11, 13-30 match Conway CDDL (key 12 intentionally skipped for protocol_version).
  - ~~C4 (BlockFetch MsgBlock tag24)~~: Already fixed in commit `12ae74cd`.

---

## Phase 1: Critical Wire Format Fixes (Blocks Correct Operation)

These issues cause immediate protocol incompatibility with Haskell nodes. Nothing else matters until these are fixed.

---

### Task 1.1: Fix TxSubmission2 Indefinite-Length Arrays

**Files:**
- Modify: `crates/torsten-network/src/protocol/txsubmission/mod.rs:109,119,127,159-162,181-184,198-201`
- Test: `crates/torsten-network/src/protocol/txsubmission/mod.rs` (inline tests)

**Context:** The CDDL spec requires all inner lists in TxSubmission2 (`txIdList`, `txList`, `txIdsAndSizes`) to use indefinite-length CBOR arrays. Torsten uses definite-length arrays for encoding and rejects indefinite arrays on decode. This breaks ALL tx propagation with Haskell nodes.

- [ ] **Step 1: Write failing test for indefinite-length encoding**

Add test in the txsubmission module:
```rust
#[test]
fn test_msg_reply_tx_ids_uses_indefinite_array() {
    let msg = TxSubmissionMessage::MsgReplyTxIds {
        blocking: false,
        ids: vec![(vec![0xAB; 32], 100u32)],
    };
    let encoded = encode_message(&msg);
    // Find the inner list start - should be 0x9F (indefinite array), not 0x81 (array of 1)
    // The exact position depends on outer encoding, but the inner list must use 0x9F...0xFF
    assert!(encoded.windows(1).any(|w| w[0] == 0x9F),
        "Inner list must use indefinite-length encoding (0x9F)");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p torsten-network -- test_msg_reply_tx_ids_uses_indefinite_array -v`
Expected: FAIL — currently uses definite-length encoding

- [ ] **Step 3: Fix encoding — change all three inner lists to indefinite-length**

At line 109 (`MsgReplyTxIds`), line 119 (`MsgRequestTxs`), line 127 (`MsgReplyTxs`):
Replace `enc.array(items.len() as u64)` with `enc.begin_array()` and add `enc.end()` after the loop.

```rust
// Line 109 — MsgReplyTxIds inner list
enc.begin_array().expect("infallible");  // was: enc.array(ids.len() as u64)
for (tx_id, size) in ids {
    // ... existing per-item encoding ...
}
enc.end().expect("infallible");  // close indefinite array

// Line 119 — MsgRequestTxs inner list
enc.begin_array().expect("infallible");
for id in ids {
    // ... existing per-item encoding ...
}
enc.end().expect("infallible");

// Line 127 — MsgReplyTxs inner list
enc.begin_array().expect("infallible");
for tx in txs {
    // ... existing per-item encoding ...
}
enc.end().expect("infallible");
```

- [ ] **Step 4: Fix decoding — accept both indefinite and definite arrays**

At lines 159-162, 181-184, 198-201: Replace the `.ok_or("indefinite array not supported")?` pattern with logic that handles both:

```rust
// Replace:
//   let len = dec.array().map_err(|e| e.to_string())?
//       .ok_or("indefinite array not supported")?;

// With:
let maybe_len = dec.array().map_err(|e| e.to_string())?;
// If definite length, use it; if indefinite (None), collect until break
let items = if let Some(len) = maybe_len {
    let mut items = Vec::with_capacity(len as usize);
    for _ in 0..len {
        // ... existing per-item decode ...
        items.push(item);
    }
    items
} else {
    let mut items = Vec::new();
    while dec.datatype().map_err(|e| e.to_string())? != minicbor::data::Type::Break {
        // ... existing per-item decode ...
        items.push(item);
    }
    dec.skip().map_err(|e| e.to_string())?; // consume break
    items
};
```

Apply this pattern to all three decode sites (MsgReplyTxIds, MsgRequestTxs, MsgReplyTxs).

- [ ] **Step 5: Fix the same pattern in PeerSharing and ChainSync**

The same `"indefinite array not supported"` rejection exists in:
- `crates/torsten-network/src/protocol/peersharing/mod.rs:83`
- `crates/torsten-network/src/protocol/chainsync/mod.rs:252`

Apply the same fix: accept both definite and indefinite arrays on decode. For encoding, check the CDDL for each protocol — PeerSharing and ChainSync may use definite arrays (verify before changing encoding).

- [ ] **Step 6: Run all network tests**

Run: `cargo test -p torsten-network -v`
Expected: All PASS

- [ ] **Step 7: Commit**

```bash
git add crates/torsten-network/src/protocol/txsubmission/mod.rs \
       crates/torsten-network/src/protocol/peersharing/mod.rs \
       crates/torsten-network/src/protocol/chainsync/mod.rs
git commit -m "fix(network): use indefinite-length CBOR arrays in TxSubmission2 and accept indefinite arrays in all protocols"
```

---

### Task 1.2: Fix ChainSync Server — Send Header, Not Full Block

**Files:**
- Modify: `crates/torsten-network/src/protocol/chainsync/server.rs:163-206`
- Modify: `crates/torsten-network/src/block_provider.rs` (or wherever `BlockProvider` trait is defined)
- Test: integration test in `crates/torsten-network/`

**Context:** N2N ChainSync `MsgRollForward` must send only the block header, not the full block. Headers are 10-100x smaller. Sending full blocks doubles bandwidth and may confuse Haskell decoders.

- [ ] **Step 1: Add header extraction utility**

Create a function to extract the header from a multi-era block CBOR. A Shelley+ block is `[header, tx_bodies, tx_witnesses, aux_data, invalid_txs]` — the header is index 0 of this array. The HFC wrapping is `[era_tag, block_content]`, so for a wrapped block: decode the outer array, get era_tag at index 0, then the inner block array at index 1, then header at inner index 0. The header should be re-wrapped as `[era_tag, tag24(header_cbor)]`.

```rust
/// Extract the header CBOR from a full block CBOR (HFC-wrapped).
/// Returns [era_tag, tag24(header_bytes)] suitable for N2N ChainSync.
pub fn extract_header_from_block(block_cbor: &[u8]) -> Result<Vec<u8>, String> {
    // Decode outer [era_tag, block_content]
    let mut dec = minicbor::Decoder::new(block_cbor);
    let outer_len = dec.array().map_err(|e| e.to_string())?;
    let era_tag = dec.u64().map_err(|e| e.to_string())?;
    // block_content is [header, tx_bodies, witnesses, aux, invalid]
    let inner_pos = dec.position();
    let inner_len = dec.array().map_err(|e| e.to_string())?;
    // header is the first element — capture its raw bytes
    let header_start = dec.position();
    dec.skip().map_err(|e| e.to_string())?; // skip header
    let header_end = dec.position();
    let header_bytes = &block_cbor[header_start..header_end];

    // Re-encode as [era_tag, tag24(header_bytes)]
    let mut enc = minicbor::encode::Encoder::new(Vec::new());
    enc.array(2).expect("infallible");
    enc.u64(era_tag).expect("infallible");
    enc.tag(minicbor::data::Tag::new(24)).expect("infallible");
    enc.bytes(header_bytes).expect("infallible");
    Ok(enc.into_writer())
}
```

- [ ] **Step 2: Write test for header extraction**

```rust
#[test]
fn test_extract_header_from_block() {
    // Construct a minimal HFC-wrapped block: [7, [header, [], [], {}, []]]
    // header = [block_no, slot, prev_hash, ...]
    let mut enc = minicbor::encode::Encoder::new(Vec::new());
    enc.array(2).unwrap();
    enc.u64(7).unwrap(); // Conway era tag
    enc.array(5).unwrap();
    // header (simplified)
    enc.array(2).unwrap();
    enc.u64(42).unwrap(); // block_no
    enc.u64(100).unwrap(); // slot
    // tx_bodies, witnesses, aux, invalid (empty)
    enc.array(0).unwrap();
    enc.array(0).unwrap();
    enc.map(0).unwrap();
    enc.array(0).unwrap();
    let block_cbor = enc.into_writer();

    let header = extract_header_from_block(&block_cbor).unwrap();
    // Should be [7, tag24(header_bytes)]
    let mut dec = minicbor::Decoder::new(&header);
    assert_eq!(dec.array().unwrap(), Some(2));
    assert_eq!(dec.u64().unwrap(), 7); // era tag preserved
}
```

- [ ] **Step 3: Run test to verify extraction works**

Run: `cargo test -p torsten-network -- test_extract_header -v`

- [ ] **Step 4: Update ChainSync server to use header extraction**

In `server.rs`, at both MsgRollForward sites (lines 163 and 197), replace:
```rust
header: block_cbor,
```
with:
```rust
header: match extract_header_from_block(&block_cbor) {
    Ok(h) => h,
    Err(e) => {
        error!("Header extraction failed for block at slot {}: {e}", slot);
        return Err(format!("header extraction failed: {e}"));
    }
},
```

- [ ] **Step 5: Run full test suite**

Run: `cargo test -p torsten-network -v`
Expected: All PASS

- [ ] **Step 6: Commit**

```bash
git add crates/torsten-network/src/protocol/chainsync/server.rs
git commit -m "fix(network): send block header (not full block) in N2N ChainSync MsgRollForward"
```

---

### Task 1.3: Fix `required_signers` Hash Length (32 -> 28 bytes)

**Files:**
- Modify: `crates/torsten-serialization/src/encode/transaction.rs:426-433`
- Test: `crates/torsten-serialization/` (inline or test module)

**Context:** CDDL requires `required_signers = nonempty_set<addr_keyhash>` where `addr_keyhash = hash28` (28 bytes). Torsten stores as `Hash32` (padded) and encodes 32 bytes. Must emit only the first 28 bytes.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_required_signers_encode_as_28_bytes() {
    let mut body = TransactionBody::default();
    body.required_signers = vec![Hash32::from([0xAB; 32])];
    let encoded = encode_transaction_body(&body);
    // The signer hash should be 28 bytes (CBOR: 0x58 0x1C + 28 bytes = 30 bytes)
    // NOT 32 bytes (CBOR: 0x58 0x20 + 32 bytes = 34 bytes)
    // Search for 0x58 0x1C (28-byte bstr header)
    assert!(encoded.windows(2).any(|w| w[0] == 0x58 && w[1] == 0x1C),
        "required_signers must encode as 28-byte addr_keyhash, not 32-byte hash");
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Fix encoding — truncate to 28 bytes**

At `encode/transaction.rs:426-433`, change:
```rust
// Before:
for hash in &body.required_signers {
    buf.extend(encode_hash32(hash));
}

// After:
for hash in &body.required_signers {
    buf.extend(encode_bytes(&hash.as_bytes()[..28])); // addr_keyhash is 28 bytes
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p torsten-serialization -v`

- [ ] **Step 5: Commit**

```bash
git add crates/torsten-serialization/src/encode/transaction.rs
git commit -m "fix(serialization): encode required_signers as 28-byte addr_keyhash per CDDL"
```

---

### Task 1.4: Fix ChainDB flush_to_immutable — Use Chain Fragment Order

**Files:**
- Modify: `crates/torsten-storage/src/chain_db.rs:548-605`
- Test: `crates/torsten-storage/src/chain_db.rs` (test module)

**Context:** `flush_to_immutable` iterates by `block_no` over the VolatileDB, which can flush non-canonical fork blocks when competing blocks exist at the same block number. Must walk the chain fragment from oldest to newest instead.

- [ ] **Step 1: Write test demonstrating the fork-block flush bug**

```rust
#[test]
fn test_flush_selects_canonical_chain_not_by_block_no() {
    // Create a ChainDB with two competing blocks at the same block_no
    let mut chain_db = ChainDB::new_test();
    let block_a = make_test_block(100, 5, HASH_A, HASH_PARENT); // slot, block_no, hash, prev
    let block_b = make_test_block(101, 5, HASH_B, HASH_PARENT); // fork block at same block_no
    // block_a is canonical (on the selected chain), block_b is a fork
    chain_db.add_block(block_a.clone());
    chain_db.add_block(block_b.clone());
    chain_db.set_tip(HASH_A); // canonical tip is block_a

    chain_db.flush_to_immutable(4); // finalize up to block_no 5 (k=1 so finalize=tip-k)

    // The immutable DB should contain block_a, NOT block_b
    let immutable_block = chain_db.immutable_db.get_block_by_number(5);
    assert_eq!(immutable_block.unwrap().hash, HASH_A);
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Rewrite flush logic to walk chain fragment**

Replace the `for block_no in start_block_no..=finalize_up_to_block_no` loop with chain fragment traversal:

```rust
pub fn flush_to_immutable(&mut self, security_param: u64) -> Result<u64, StorageError> {
    // Walk the canonical chain from its oldest entry, collecting hashes to flush
    let volatile_tip = self.volatile_db.tip();
    let Some(tip) = volatile_tip else { return Ok(0) };

    let finalize_up_to_block_no = tip.block_no.saturating_sub(security_param);
    if finalize_up_to_block_no == 0 { return Ok(0); }

    // Walk backward from tip to collect the canonical chain
    let mut chain_hashes: Vec<Hash32> = Vec::new();
    let mut current_hash = tip.hash;
    loop {
        let block = self.volatile_db.get_block(&current_hash);
        let Some(block) = block else { break };
        if block.block_no <= finalize_up_to_block_no {
            chain_hashes.push(block.hash);
        }
        if block.block_no <= self.immutable_tip_block_no() + 1 {
            break;
        }
        current_hash = block.prev_hash;
    }
    chain_hashes.reverse(); // oldest first

    // Flush in chain order
    let mut flushed = 0;
    for hash in &chain_hashes {
        if let Some(block) = self.volatile_db.get_block(hash) {
            self.immutable_db.append_block(&block)?;
            flushed += 1;
        }
    }

    // Remove flushed blocks from volatile
    // ... (existing cleanup logic)

    Ok(flushed)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p torsten-storage -v`

- [ ] **Step 5: Commit**

```bash
git add crates/torsten-storage/src/chain_db.rs
git commit -m "fix(storage): flush_to_immutable walks canonical chain fragment, not block_no scan"
```

---

## Phase 2: Consensus Correctness

Issues that affect chain selection, block validation, and leader election correctness.

---

### Task 2.1: Fix Opcert Counter Initialization for First-Seen Pools

**Files:**
- Modify: `crates/torsten-consensus/src/praos.rs:574-648`
- Test: `crates/torsten-consensus/src/praos.rs` (test module)

**Context:** Haskell initializes `currentIssueNo = Just 0` when a pool is in the stake distribution but has no counter entry. Torsten accepts any counter value for a pool's first block, defeating replay protection.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_first_block_from_pool_rejects_high_counter() {
    let mut praos = OuroborosPraos::new_test();
    // Pool is in stake distribution but has no opcert counter entry
    praos.register_pool(POOL_ID, vrf_key, stake);
    assert!(praos.opcert_counters.get(&POOL_ID).is_none());

    // First block claims counter = 50 — should be rejected
    let header = make_header(pool_id: POOL_ID, opcert_counter: 50);
    let result = praos.check_opcert_counter(&header, &POOL_ID);
    assert!(result.is_err(), "First block from pool with counter > 1 must be rejected");

    // First block claims counter = 0 — should be accepted
    let header = make_header(pool_id: POOL_ID, opcert_counter: 0);
    let result = praos.check_opcert_counter(&header, &POOL_ID);
    assert!(result.is_ok());

    // First block claims counter = 1 — should be accepted
    let header = make_header(pool_id: POOL_ID, opcert_counter: 1);
    let result = praos.check_opcert_counter(&header, &POOL_ID);
    assert!(result.is_ok());
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Fix — initialize counter to 0 for known pools**

In `check_opcert_counter`, after the `if let Some(&m)` branch, add:
```rust
if let Some(&m) = self.opcert_counters.get(&pool_id) {
    // ... existing counter regression / over-increment checks ...
} else if issuer_info.is_some() {
    // Pool is in stake distribution but first block ever seen
    // Haskell initializes currentIssueNo = Just 0
    let m: u64 = 0;
    if n > m + 1 {
        return Err(ConsensusError::OpcertCounterTooLarge {
            pool_id,
            expected_max: m + 1,
            actual: n,
        });
    }
    self.opcert_counters.insert(pool_id, n);
} else {
    // Unknown pool — not in stake distribution
    return Err(ConsensusError::UnregisteredPool { pool_id });
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p torsten-consensus -v`

- [ ] **Step 5: Commit**

```bash
git add crates/torsten-consensus/src/praos.rs
git commit -m "fix(consensus): initialize opcert counter to 0 for first-seen pools per Haskell spec"
```

---

### Task 2.2: Fix Chain Selection Tiebreaker (Same-Pool Requires Same-Slot)

**Files:**
- Modify: `crates/torsten-consensus/src/chain_selection.rs:261-319`
- Test: `crates/torsten-consensus/src/chain_selection.rs` (test module)

**Context:** Haskell's `issueNoArmed` fires only when BOTH same issuer AND same slot. Torsten fires for all same-pool pairs regardless of slot, using opcert counter instead of VRF for different-slot same-pool forks.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_same_pool_different_slot_uses_vrf_not_opcert() {
    // Two blocks from the same pool at different slots
    let block_a = make_chain_entry(pool: POOL_A, slot: 100, opcert_seq: 5, vrf: VRF_HIGH);
    let block_b = make_chain_entry(pool: POOL_A, slot: 101, opcert_seq: 3, vrf: VRF_LOW);
    // Haskell: different slots → VRF tiebreak → lower VRF wins → block_b wins
    // Torsten (buggy): same pool → opcert tiebreak → higher opcert wins → block_a wins
    let result = praos_tiebreak(&block_a, &block_b);
    assert_eq!(result, Ordering::Less, "Same pool, different slot: should use VRF, not opcert");
}
```

- [ ] **Step 2: Run test to verify it fails**

- [ ] **Step 3: Fix — add slot equality check**

At `chain_selection.rs:293`, change:
```rust
// Before:
if current_pool == candidate_pool {
    // same pool: higher opcert wins

// After:
if current_pool == candidate_pool && current.slot == candidate.slot {
    // same pool AND same slot: higher opcert wins (issueNoArmed)
```

When same pool but different slot, fall through to the VRF comparison.

- [ ] **Step 4: Run tests**

Run: `cargo test -p torsten-consensus -v`

- [ ] **Step 5: Commit**

```bash
git add crates/torsten-consensus/src/chain_selection.rs
git commit -m "fix(consensus): chain selection tiebreaker requires same-slot for opcert comparison"
```

---

### Task 2.3: Add Block Body Size and Header Size Limit Enforcement

**Files:**
- Modify: `crates/torsten-consensus/src/praos.rs` (in `validate_header_full`)
- Test: inline tests

**Context:** Haskell's `envelopeChecks` validates both header size and body size against protocol parameter limits in the consensus layer. Torsten has a non-fatal warning in the ledger layer. Per Haskell architecture, this belongs in consensus only — the ledger layer assumes blocks already pass envelope checks.

- [ ] **Step 1: Write test for body size rejection**

```rust
#[test]
fn test_oversized_block_body_rejected() {
    let header = make_header_with_body_size(200_000); // exceeds max_block_body_size of 90112
    let params = ProtocolParameters { max_block_body_size: 90112, ..Default::default() };
    let result = validate_envelope(&header, &params);
    assert!(result.is_err());
}
```

- [ ] **Step 2: Add envelope checks in `validate_header_full`**

```rust
// Block body size check (matches Haskell envelopeChecks)
if header.body_size > protocol_params.max_block_body_size {
    return Err(ConsensusError::BlockBodyTooLarge {
        actual: header.body_size,
        limit: protocol_params.max_block_body_size,
    });
}
// Header size check (if header raw_cbor length is available)
if let Some(header_size) = header.raw_header_size() {
    if header_size > protocol_params.max_block_header_size {
        return Err(ConsensusError::BlockHeaderTooLarge {
            actual: header_size,
            limit: protocol_params.max_block_header_size,
        });
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test --all -v`

- [ ] **Step 4: Commit**

```bash
git add crates/torsten-consensus/src/praos.rs
git commit -m "fix(consensus): enforce block body size and header size limits per envelopeChecks"
```

---

### Task 2.4: Fix Leader Schedule to Use Rational Arithmetic

**Files:**
- Modify: `crates/torsten-consensus/src/slot_leader.rs:117`
- Test: `crates/torsten-consensus/src/slot_leader.rs` (test module)

**Context:** `compute_leader_schedule` calls `is_slot_leader` (f64 path) instead of `is_slot_leader_rational`. This can cause precision boundary divergences where Torsten computes leadership differently than Haskell.

- [ ] **Step 1: Change `compute_leader_schedule` to use rational path**

At `slot_leader.rs:117`, replace:
```rust
// Before:
is_slot_leader(vrf_output, relative_stake, active_slot_coeff)

// After:
is_slot_leader_rational(vrf_output, &pool_stake, &total_stake, &active_slot_coeff_rational)
```

Ensure `pool_stake` and `total_stake` are passed as exact integer values, not f64.

- [ ] **Step 2: Cache `activeSlotLog` value**

Cache the `ln(1-f)` fixed-point result on the `OuroborosPraos` struct to avoid recomputing per-slot.

- [ ] **Step 3: Run tests**

Run: `cargo test -p torsten-consensus -v`

- [ ] **Step 4: Commit**

```bash
git add crates/torsten-consensus/src/slot_leader.rs
git commit -m "fix(consensus): use exact rational arithmetic for leader schedule computation"
```

---

## Phase 3: Ledger Validation Completeness

Missing validation rules that cause Torsten to accept transactions/blocks that Haskell rejects.

---

### Task 3.1: Add Stake Deregistration Balance Check

**Files:**
- Modify: `crates/torsten-ledger/src/state/certificates.rs:112-152`
- Modify: `crates/torsten-ledger/src/validation/phase1.rs` or `validation/conway.rs`
- Test: `crates/torsten-ledger/src/validation/` test module

**Context:** `StakeKeyHasNonZeroAccountBalanceDELEG` — deregistration with non-zero reward balance must be rejected. Currently, both `StakeDeregistration` (line 112) and `ConwayStakeDeregistration` (line 140) unconditionally remove without checking.

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_stake_deregistration_rejected_with_nonzero_balance() {
    let mut state = LedgerState::new_test();
    let cred = StakeCredential::from_keyhash(HASH_A);
    state.reward_accounts.insert(cred.clone(), Lovelace(1_000_000));
    state.delegations.insert(cred.clone(), POOL_ID);

    let cert = Certificate::StakeDeregistration(cred.clone());
    let result = state.validate_certificate(&cert);
    assert!(matches!(result, Err(ValidationError::StakeKeyHasNonZeroBalance { .. })));
}
```

- [ ] **Step 2: Add validation check**

In the certificate validation path (phase1.rs or conway.rs), before processing deregistration:
```rust
Certificate::StakeDeregistration(cred) | Certificate::ConwayStakeDeregistration { credential: cred, .. } => {
    if let Some(&balance) = self.reward_accounts.get(cred) {
        if balance.0 > 0 {
            return Err(ValidationError::StakeKeyHasNonZeroBalance {
                credential: cred.clone(),
                balance,
            });
        }
    }
}
```

- [ ] **Step 3: Add ConwayUnRegCert refund amount validation**

For `ConwayStakeDeregistration`, also validate the refund amount:
```rust
if let Some(declared_refund) = refund {
    let expected_deposit = self.protocol_params.key_deposit;
    if declared_refund != expected_deposit {
        return Err(ValidationError::RefundIncorrect {
            declared: declared_refund,
            expected: expected_deposit,
        });
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p torsten-ledger -v`

- [ ] **Step 5: Commit**

```bash
git add crates/torsten-ledger/
git commit -m "fix(ledger): reject stake deregistration with non-zero reward balance"
```

---

### Task 3.2a: Add Stake Registration/Delegation Validation Checks

**Files:**
- Modify: `crates/torsten-ledger/src/validation/phase1.rs` or `validation/conway.rs`
- Test: test module

**Context:** Missing checks for duplicate stake registration and delegation to unregistered pools/DReps.

- [ ] **Step 1: Write test for duplicate registration rejection**

```rust
#[test]
fn test_duplicate_stake_registration_rejected() {
    let mut state = LedgerState::new_test();
    let cred = StakeCredential::from_keyhash(HASH_A);
    state.reward_accounts.insert(cred.clone(), Lovelace(0));
    let cert = Certificate::StakeRegistration(cred.clone());
    let result = state.validate_certificate(&cert);
    assert!(matches!(result, Err(ValidationError::StakeKeyAlreadyRegistered { .. })));
}
```

- [ ] **Step 2: Add duplicate stake registration rejection**

```rust
Certificate::StakeRegistration(cred) | Certificate::ConwayStakeRegistration { credential: cred, .. } => {
    if self.reward_accounts.contains_key(cred) {
        return Err(ValidationError::StakeKeyAlreadyRegistered { credential: cred.clone() });
    }
}
```

- [ ] **Step 3: Add delegation to unregistered pool rejection**

```rust
Certificate::StakeDelegation { credential: _, pool_id } => {
    if !self.pool_params.contains_key(pool_id) {
        return Err(ValidationError::DelegateePoolNotRegistered { pool_id: *pool_id });
    }
}
```

- [ ] **Step 4: Add DRep re-registration rejection**

```rust
Certificate::RegDRep { credential: cred, .. } => {
    if self.governance.dreps.contains_key(&credential_to_hash(cred)) {
        return Err(ValidationError::DRepAlreadyRegistered { credential: cred.clone() });
    }
}
```

- [ ] **Step 5: Write tests for delegation checks, run all tests**

Run: `cargo test -p torsten-ledger -v`

- [ ] **Step 6: Commit**

```bash
git add crates/torsten-ledger/
git commit -m "fix(ledger): reject duplicate stake/DRep registration and delegation to unregistered pools"
```

---

### Task 3.2b: Add Pool Registration Validation Checks

**Files:**
- Modify: `crates/torsten-ledger/src/validation/phase1.rs` or `validation/conway.rs`
- Modify: `crates/torsten-ledger/src/state/mod.rs` (add `vrf_key_to_pool` field)
- Test: test module

**Context:** Missing pool-specific checks: VRF key deduplication (Conway+), minimum pool cost, pool reward account network ID.

- [ ] **Step 1: Add `vrf_key_to_pool: HashMap<Hash32, Hash28>` to LedgerState**

Maintain this map in `process_certificate` for `PoolRegistration`:
```rust
self.vrf_key_to_pool.insert(pool_params.vrf_keyhash, pool_id);
```

- [ ] **Step 2: Add pool VRF key deduplication (Conway+)**

```rust
if self.protocol_params.protocol_version_major >= 9 {
    if let Some(&existing_pool) = self.vrf_key_to_pool.get(&pool_params.vrf_keyhash) {
        if existing_pool != pool_id {
            return Err(ValidationError::VrfKeyAlreadyRegistered {
                vrf_key: pool_params.vrf_keyhash,
                existing_pool,
            });
        }
    }
}
```

- [ ] **Step 3: Add minimum pool cost check**

```rust
if pool_params.cost < self.protocol_params.min_pool_cost {
    return Err(ValidationError::PoolCostTooLow {
        declared: pool_params.cost,
        minimum: self.protocol_params.min_pool_cost,
    });
}
```

- [ ] **Step 4: Add pool reward account network ID check (Alonzo+)**

```rust
if self.protocol_params.protocol_version_major >= 5 {
    let reward_network = pool_params.reward_account.network_id();
    if reward_network != self.expected_network {
        return Err(ValidationError::WrongNetworkPoolReward {
            expected: self.expected_network,
            actual: reward_network,
        });
    }
}
```

- [ ] **Step 5: Write tests for each check, run all tests**

Run: `cargo test -p torsten-ledger -v`

- [ ] **Step 6: Commit**

```bash
git add crates/torsten-ledger/
git commit -m "fix(ledger): add pool VRF key dedup, min cost, and reward account network checks"
```

---

### Task 3.2c: Add Committee Certificate Validation Checks

**Files:**
- Modify: `crates/torsten-ledger/src/validation/phase1.rs` or `validation/conway.rs`
- Test: test module

**Context:** CC hot auth for unknown/resigned members is not rejected.

- [ ] **Step 1: Add CC hot auth checks**

```rust
Certificate::CommitteeHotAuth { cold_key, .. } => {
    // Check member is known
    if !self.governance.committee_expiration.contains_key(&credential_to_hash(cold_key)) {
        return Err(ValidationError::CommitteeIsUnknown { cold_key: cold_key.clone() });
    }
    // Check member has not resigned
    if self.governance.committee_resigned.contains(&credential_to_hash(cold_key)) {
        return Err(ValidationError::CommitteeHasPreviouslyResigned { cold_key: cold_key.clone() });
    }
}
```

- [ ] **Step 2: Write tests**

- [ ] **Step 3: Run tests and commit**

```bash
git add crates/torsten-ledger/
git commit -m "fix(ledger): reject CC hot auth for unknown or resigned committee members"
```

---

### Task 3.3: Add Phase-1 Network ID and Auxiliary Data Checks

**Files:**
- Modify: `crates/torsten-ledger/src/validation/phase1.rs:224-232,492-509`
- Test: test module

- [ ] **Step 1: Fix auxiliary data hash content verification**

At lines 224-232, after checking presence/absence match, add content hash comparison:
```rust
if let (Some(declared_hash), Some(aux_data)) = (&tx.body.auxiliary_data_hash, &tx.auxiliary_data) {
    let computed_hash = blake2b_256(&aux_data.raw_cbor);
    if computed_hash != *declared_hash {
        errors.push(ValidationError::AuxiliaryDataHashMismatch {
            declared: *declared_hash,
            computed: computed_hash,
        });
    }
}
```

- [ ] **Step 2: Add unconditional output address network ID check**

Add a check that runs regardless of `tx.body.network_id`:
```rust
// Check ALL output addresses match the expected network (unconditional)
for output in &tx.body.outputs {
    let addr_network = output.address.network_id();
    if addr_network != expected_network {
        errors.push(ValidationError::WrongNetwork {
            expected: expected_network,
            actual: addr_network,
            address: output.address.clone(),
        });
    }
}
```

- [ ] **Step 3: Add withdrawal network ID check**

```rust
for (reward_addr, _) in &tx.body.withdrawals {
    let addr_network = reward_addr.network_id();
    if addr_network != expected_network {
        errors.push(ValidationError::WrongNetworkWithdrawal {
            expected: expected_network,
            actual: addr_network,
        });
    }
}
```

- [ ] **Step 4: Write tests for each check**

- [ ] **Step 5: Run tests**

Run: `cargo test -p torsten-ledger -v`

- [ ] **Step 6: Commit**

```bash
git add crates/torsten-ledger/src/validation/phase1.rs
git commit -m "fix(ledger): add aux data hash content verification and unconditional network ID checks"
```

---

### Task 3.4: Fix Treasury Donation Timing

**Files:**
- Modify: `crates/torsten-ledger/src/state/apply.rs:946-949`
- Modify: `crates/torsten-ledger/src/state/mod.rs` (add `pending_donations` field)
- Modify: `crates/torsten-ledger/src/state/epoch.rs` (apply donations at epoch boundary)
- Test: test module

**Context:** Haskell holds donations in `utxosDonation` until epoch boundary. Torsten credits immediately, making mid-epoch `currentTreasuryValue` checks diverge.

- [ ] **Step 1: Add `pending_donations: Lovelace` field to LedgerState**

- [ ] **Step 2: Buffer donations instead of applying immediately**

At `apply.rs:947-949`, change:
```rust
// Before:
if let Some(donation) = tx.body.donation {
    self.treasury += donation;
}

// After:
if let Some(donation) = tx.body.donation {
    self.pending_donations += donation;
}
```

- [ ] **Step 3: Apply donations at epoch boundary**

In `epoch.rs` `process_epoch_transition`, add:
```rust
self.treasury += self.pending_donations;
self.pending_donations = Lovelace(0);
```

- [ ] **Step 4: Write test**

- [ ] **Step 5: Run tests and commit**

```bash
git commit -m "fix(ledger): buffer treasury donations until epoch boundary per Haskell spec"
```

---

### Task 3.5: Add Governance Validation Checks

**Files:**
- Modify: `crates/torsten-ledger/src/state/governance.rs`
- Modify: `crates/torsten-ledger/src/validation/conway.rs`
- Test: test module

**Context:** Multiple missing governance checks: `actionWellFormed`, bootstrap phase restrictions, `pvCanFollow`, proposal deposit validation, return address registration, unelected CC votes, prev_action_id at submission.

- [ ] **Step 1: Add bootstrap phase proposal restriction**

```rust
fn validate_proposal_bootstrap(proposal: &GovAction, protocol_version: u64) -> Result<(), ValidationError> {
    if protocol_version == 9 { // Bootstrap phase
        match proposal {
            GovAction::NoConfidence { .. } |
            GovAction::UpdateCommittee { .. } |
            GovAction::NewConstitution { .. } |
            GovAction::InfoAction => Ok(()),
            _ => Err(ValidationError::DisallowedProposalDuringBootstrap),
        }
    } else {
        Ok(())
    }
}
```

- [ ] **Step 2: Add `pvCanFollow` for hard fork proposals**

```rust
GovAction::HardForkInitiation { protocol_version: proposed, .. } => {
    let current = &self.protocol_params.protocol_version;
    let can_follow = (proposed.major == current.major + 1 && proposed.minor == 0)
        || (proposed.major == current.major && proposed.minor > current.minor);
    if !can_follow {
        return Err(ValidationError::ProposalCantFollow { current: *current, proposed: *proposed });
    }
}
```

- [ ] **Step 3: Add proposal deposit validation**

```rust
if proposal.deposit != self.protocol_params.gov_action_deposit {
    return Err(ValidationError::ProposalDepositIncorrect {
        declared: proposal.deposit,
        expected: self.protocol_params.gov_action_deposit,
    });
}
```

- [ ] **Step 4: Add return address registration check (outside bootstrap)**

```rust
if protocol_version.major >= 10 {
    if !self.reward_accounts.contains_key(&proposal.return_addr.credential) {
        return Err(ValidationError::ProposalReturnAccountDoesNotExist);
    }
}
```

- [ ] **Step 5: Add unelected CC vote rejection (protocol >= 10)**

In `process_vote`, when voter is CC:
```rust
if protocol_version.major >= 10 {
    let cold_key = self.governance.committee_hot_to_cold.get(&hot_credential);
    if cold_key.is_none() || !self.governance.committee_expiration.contains_key(cold_key.unwrap()) {
        return Err(ValidationError::UnelectedCommitteeVoter { credential: hot_credential });
    }
}
```

- [ ] **Step 6: Add prev_action_id validation at proposal submission**

In `process_proposal`, validate prev_action_id references an active proposal or the last enacted action of the same purpose.

- [ ] **Step 7: Fix DRep voting power — use mark snapshot**

Capture DRep distribution at epoch boundary:
```rust
// In process_epoch_transition:
self.drep_distribution_snapshot = self.compute_drep_distribution();

// In governance ratification, use snapshot instead of live state:
let drep_power = &self.drep_distribution_snapshot;
```

- [ ] **Step 8: Write tests for each governance check**

- [ ] **Step 9: Run tests and commit**

```bash
git commit -m "fix(ledger): add governance validation checks (bootstrap, pvCanFollow, deposits, CC votes, DRep snapshot)"
```

---

### Task 3.6: Fix MIR Source Pot Validation

**Files:**
- Modify: `crates/torsten-ledger/src/state/certificates.rs:464-490`
- Test: test module

- [ ] **Step 1: Add pot balance check before MIR transfer**

```rust
MIRTarget::OtherAccountingPot(coin) => {
    let source_balance = match source {
        MIRPot::Reserves => self.reserves,
        MIRPot::Treasury => self.treasury,
    };
    if coin > source_balance {
        return Err(ValidationError::InsufficientMIRSourcePot {
            source: source.clone(),
            requested: coin,
            available: source_balance,
        });
    }
    // ... existing transfer logic ...
}
```

- [ ] **Step 2: Write test and commit**

---

## Phase 4: Storage & Node Correctness

---

### Task 4.1: Wire In `startup.rs` Recovery Sequence

**Files:**
- Modify: `crates/torsten-node/src/node/mod.rs:382+` (replace legacy startup path)
- Modify: `crates/torsten-node/src/startup.rs` (remove `#[allow(dead_code)]`)
- Test: integration test

**Context:** The correct 6-step recovery (load snapshot -> gap replay -> volatile replay) is fully implemented in `startup.rs` but dead code. The current `Node::new()` uses legacy inline replay.

- [ ] **Step 1: Remove `#[allow(dead_code)]` from `startup.rs`**

- [ ] **Step 2: Replace legacy startup path in `Node::new()`**

Replace the inline `ledger-snapshot.bin` loading + block replay with a call to `startup::recover()`:
```rust
let (ledger_state, chain_db) = startup::recover(
    &db_path,
    &genesis_config,
    security_param,
)?;
```

- [ ] **Step 3: Wire LedgerSeq for O(k) rollback support**

Replace `reset_ledger_and_replay` (deprecated) with `LedgerSeq::rollback()`.

- [ ] **Step 4: Test startup recovery with snapshot + gap replay**

- [ ] **Step 5: Commit**

```bash
git commit -m "feat(node): wire in startup.rs recovery sequence, replace legacy replay path"
```

---

### Task 4.2: Write Primary Index Files for ImmutableDB

**Files:**
- Modify: `crates/torsten-storage/src/immutable_db.rs`
- Test: test module

**Context:** Haskell requires `.primary` files for slot-based block lookups. Torsten only writes `.secondary`. Without `.primary`, the chunk files are incompatible with Haskell tools.

- [ ] **Step 1: Implement primary index format**

The primary index is one `SecondaryOffset` (u32, 4 bytes, big-endian) per relative slot within the chunk. For slot S within epoch E, the entry at `relative_slot = S - first_slot_of_epoch` gives the byte offset into the `.secondary` file where that slot's entry begins. Empty slots have the same offset as the next filled slot (or the end-of-file offset).

```rust
pub fn write_primary_index(
    path: &Path,
    epoch_size: u64,
    secondary_entries: &[SecondaryEntry],
) -> Result<(), StorageError> {
    let num_slots = epoch_size as usize + 1; // +1 for sentinel
    // Use Option to distinguish "offset 0 (has block)" from "unfilled (no block)"
    let mut primary: Vec<Option<u32>> = vec![None; num_slots];
    // Fill primary index: each relative slot maps to a secondary offset
    let entry_size: u32 = 56; // bytes per secondary entry
    for (i, entry) in secondary_entries.iter().enumerate() {
        let relative_slot = (entry.slot % epoch_size) as usize;
        primary[relative_slot] = Some(i as u32 * entry_size);
    }
    // Sentinel: end-of-file offset
    let eof_offset = secondary_entries.len() as u32 * entry_size;
    // Fill gaps: empty slots get the next filled slot's offset (backwards pass)
    let mut next_offset = eof_offset;
    let resolved: Vec<u32> = primary.iter().rev().map(|slot| {
        match slot {
            Some(offset) => { next_offset = *offset; *offset }
            None => next_offset,
        }
    }).collect::<Vec<_>>().into_iter().rev().collect();
    // Write to file
    let mut file = BufWriter::new(File::create(path)?);
    for offset in &resolved {
        file.write_all(&offset.to_be_bytes())?;
    }
    file.flush()?;
    Ok(())
}
```

- [ ] **Step 2: Call `write_primary_index` in `finalize_chunk`**

- [ ] **Step 3: Write test**

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(storage): write .primary index files for ImmutableDB Haskell compatibility"
```

---

### Task 4.3: Fix Mithril Import — Per-Epoch Chunks

**Files:**
- Modify: `crates/torsten-node/src/mithril.rs`
- Test: integration test

**Context:** Mithril import appends all blocks to a single chunk. Must call `finalize_chunk()` at epoch boundaries during import.

- [ ] **Step 1: Track epoch boundaries during import**

```rust
let mut current_epoch = None;
for block in blocks {
    let block_epoch = block.slot / epoch_size;
    if let Some(prev_epoch) = current_epoch {
        if block_epoch > prev_epoch {
            // Epoch boundary crossed — finalize the current chunk
            immutable_db.finalize_chunk()?;
        }
    }
    current_epoch = Some(block_epoch);
    immutable_db.append_block(&block)?;
}
```

- [ ] **Step 2: Write primary index for each finalized chunk**

- [ ] **Step 3: Test that import produces per-epoch chunks**

- [ ] **Step 4: Commit**

```bash
git commit -m "fix(mithril): create per-epoch chunk files during snapshot import"
```

---

### Task 4.4: Add SIGTERM Handler and Shutdown Persistence

**Files:**
- Modify: `crates/torsten-node/src/node/mod.rs:1038-1068`
- Test: manual verification

**Context:** `node/mod.rs` already has SIGTERM handling (lines 1047-1057), but shutdown must guarantee `persist()` is called on ChainDB and a ledger snapshot is saved.

- [ ] **Step 1: Verify SIGTERM handler exists and is correct**

Read the shutdown handler code at lines 1038-1068 to confirm both SIGINT and SIGTERM are caught.

- [ ] **Step 2: Add explicit `chain_db.persist()` call in shutdown path**

After `flush_all_to_immutable`, add:
```rust
chain_db.persist()?; // flush active chunk's secondary index to disk
```

- [ ] **Step 3: Ensure ledger snapshot is saved on shutdown**

```rust
if let Err(e) = ledger_state.save_snapshot(&db_path) {
    error!("Failed to save ledger snapshot on shutdown: {}", e);
}
```

- [ ] **Step 4: Commit**

```bash
git commit -m "fix(node): ensure ChainDB persist and ledger snapshot on shutdown"
```

---

### Task 4.5: Improve Mempool Revalidation

**Files:**
- Modify: `crates/torsten-node/src/node/sync.rs:1301-1352`
- Modify: `crates/torsten-mempool/src/lib.rs`
- Test: test module

**Context:** Post-block revalidation only checks hash/input conflicts and TTL. Haskell runs full `applyTx` for every remaining mempool transaction against the new ticked ledger state.

- [ ] **Step 1: Add full revalidation method to mempool**

```rust
/// Revalidate all mempool transactions against the given ledger state.
/// Removes any that fail validation.
pub fn revalidate_against_ledger(&mut self, ledger: &LedgerState) -> Vec<Hash32> {
    let mut removed = Vec::new();
    let txs: Vec<_> = self.iter_transactions().collect();
    for (hash, tx) in txs {
        if let Err(_) = ledger.validate_transaction(&tx) {
            self.remove_by_hash(&hash);
            removed.push(hash);
        }
    }
    removed
}
```

- [ ] **Step 2: Call full revalidation after block application**

In `sync.rs`, after applying a block to the ledger, replace the partial check with:
```rust
let removed = mempool.revalidate_against_ledger(&ledger_state);
if !removed.is_empty() {
    debug!("Revalidated mempool: removed {} invalid txs", removed.len());
}
```

- [ ] **Step 3: Make mempool capacity dynamic**

```rust
pub fn update_capacity_from_params(&mut self, params: &ProtocolParameters) {
    self.config.max_bytes = params.max_block_body_size as usize * 2;
    // Update ExUnit limits similarly
}
```

- [ ] **Step 4: Write tests and commit**

```bash
git commit -m "fix(mempool): full ledger revalidation after block application, dynamic capacity"
```

---

### Task 4.6: Validate Forged Blocks Before Announcement

**Files:**
- Modify: `crates/torsten-node/src/forge.rs` or `node/mod.rs` (wherever forged block is announced)
- Test: test module

- [ ] **Step 1: Add ledger validation step after forging**

After `forge_block()` returns, apply the block to a clone of the ledger state:
```rust
let forged_block = forge_block(...)?;
// Validate by applying to ledger (use a scratch copy)
let mut validation_ledger = ledger_state.clone();
validation_ledger.apply_block(&forged_block, ValidateAll)?;
// Only after successful validation, apply to real ledger and announce
ledger_state.apply_block(&forged_block, ValidateAll)?;
chain_db.add_block(&forged_block)?;
block_announcement_tx.send(forged_block)?;
```

- [ ] **Step 2: Write test and commit**

```bash
git commit -m "fix(node): validate forged blocks against ledger before announcement"
```

---

### Task 4.7: Read Protocol Version from Ledger State for Forging

**Files:**
- Modify: `crates/torsten-node/src/forge.rs:235-245`

- [ ] **Step 1: Replace hardcoded protocol version with live value**

```rust
// Before:
protocol_version: ProtocolVersion { major: 9, minor: 0 },

// After — in forge_block(), read from ledger state:
protocol_version: ledger_state.protocol_params.protocol_version.clone(),
```

- [ ] **Step 2: Similarly for max_block_body_size**

```rust
// Use live parameter:
max_block_body_size: ledger_state.protocol_params.max_block_body_size,
```

- [ ] **Step 3: Commit**

```bash
git commit -m "fix(node): read protocol version and block size from live ledger state for forging"
```

---

## Phase 5: Network Behavioral Fixes

---

### Task 5.1: Fix Handshake MsgRefuse Encoding

**Files:**
- Modify: `crates/torsten-network/src/handshake/mod.rs:243-257`

- [ ] **Step 1: Fix VersionMismatch encoding**

The CDDL for `refuseReasonVersionMismatch` is `[0, [*versionNumber]]`:
```rust
// Encode MsgRefuse for VersionMismatch
enc.array(2).expect("infallible"); // outer [2, refuseReason]
enc.u8(2).expect("infallible"); // MsgRefuse tag
enc.array(2).expect("infallible"); // refuseReason = [tag, data]
enc.u8(0).expect("infallible"); // tag 0 = VersionMismatch
enc.array(supported_versions.len() as u64).expect("infallible"); // [*versionNumber]
for v in &supported_versions {
    enc.u16(*v).expect("infallible");
}
```

- [ ] **Step 2: Write test and commit**

```bash
git commit -m "fix(network): correct MsgRefuse VersionMismatch encoding per CDDL"
```

---

### Task 5.2: Fix BlockFetch Server MAX_RANGE_SLOTS

**Files:**
- Modify: `crates/torsten-network/src/protocol/blockfetch/server.rs:19`

- [ ] **Step 1: Remove or increase the artificial limit**

```rust
// Remove the constant and the associated check, or set to a much larger value:
pub const MAX_RANGE_SLOTS: u64 = 432_000; // one full Conway epoch
```

Or better: remove the slot-span limit entirely and use byte-count limits instead (matching Haskell).

- [ ] **Step 2: Commit**

```bash
git commit -m "fix(network): remove artificial MAX_RANGE_SLOTS limit on BlockFetch server"
```

---

### Task 5.3: Fix TxSubmission Server Ack Logic

**Files:**
- Modify: `crates/torsten-network/src/protocol/txsubmission/server.rs:69-76`

- [ ] **Step 1: Track fetched-and-processed tx IDs separately from offered IDs**

Only ack IDs that have been received via `MsgReplyTxs` and successfully processed, not all offered IDs.

- [ ] **Step 2: Commit**

```bash
git commit -m "fix(network): only ack tx IDs after receiving and processing tx bodies"
```

---

### Task 5.4: Fix ChainSync Server Timeout

**Files:**
- Modify: `crates/torsten-network/src/protocol/chainsync/server.rs:192`

- [ ] **Step 1: Reduce timeout to match Haskell**

```rust
// Before:
let timeout = Duration::from_secs(135);
// After:
let timeout = Duration::from_millis(3373); // matches Haskell chainSyncIdleTimeout
```

Note: This is the idle timeout for waiting for new blocks at tip, not for client requests.

- [ ] **Step 2: Commit**

```bash
git commit -m "fix(network): set ChainSync server idle timeout to 3373ms per Haskell config"
```

---

### Task 5.5: Fix Mux Ingress Byte Counter

**Files:**
- Modify: `crates/torsten-network/src/mux/ingress.rs:109`

- [ ] **Step 1: Add decrement when receiver drains the channel**

Track consumed bytes and decrement `route.buffered` when data is read from the channel, not just when it's written. This may require changing the channel architecture to include a callback or using an `AtomicUsize` counter that the receiver decrements.

- [ ] **Step 2: Commit**

```bash
git commit -m "fix(network): decrement ingress byte counter when channel is drained"
```

---

## Phase 6: Encoding Canonicality & Remaining Items

Lower-priority items that improve spec compliance but don't block correct operation.

---

### Task 6.1: Use Tag 258 for CBOR Sets

**Files:**
- Modify: `crates/torsten-serialization/src/encode/transaction.rs`

- [ ] **Step 1: Add `encode_tagged_set` helper**

```rust
fn encode_tagged_set<T>(items: &[T], encode_item: impl Fn(&T) -> Vec<u8>) -> Vec<u8> {
    let mut buf = encode_tag(258);
    buf.extend(encode_array_header(items.len()));
    // Sort items for canonical encoding
    let mut encoded_items: Vec<Vec<u8>> = items.iter().map(&encode_item).collect();
    encoded_items.sort();
    for item in encoded_items {
        buf.extend(item);
    }
    buf
}
```

- [ ] **Step 2: Apply to inputs, collateral, reference_inputs, certificates**

- [ ] **Step 3: Commit**

```bash
git commit -m "fix(serialization): use CBOR tag 258 for set-typed fields per Conway CDDL"
```

---

### Task 6.2: Use Conway Map Format for Redeemers

**Files:**
- Modify: `crates/torsten-serialization/src/encode/transaction.rs:186-192`
- Modify: `crates/torsten-serialization/src/encode/script.rs:167` (empty redeemers)

- [ ] **Step 1: Change redeemer encoding to map format**

```rust
// Conway map format: {+ [tag, index] => [data, ex_units]}
buf.extend(encode_map_header(ws.redeemers.len()));
for r in &ws.redeemers {
    // Key: [tag, index]
    buf.extend(encode_array_header(2));
    buf.extend(encode_uint(r.tag as u64));
    buf.extend(encode_uint(r.index as u64));
    // Value: [data, ex_units]
    buf.extend(encode_array_header(2));
    buf.extend(&r.data_cbor);
    buf.extend(encode_ex_units(&r.ex_units));
}
```

- [ ] **Step 2: Fix empty redeemers to use `0xA0` (empty map)**

At `script.rs:167`, ensure the empty case uses `0xA0` consistently.

- [ ] **Step 3: Commit**

```bash
git commit -m "fix(serialization): use Conway map format for redeemers encoding"
```

---

### Task 6.3: Add VolatileDB Successor Map

**Files:**
- Modify: `crates/torsten-storage/src/volatile_db.rs`

- [ ] **Step 1: Add `successor_map: HashMap<Hash32, HashSet<Hash32>>`**

Maintain on `add_block`:
```rust
self.successor_map.entry(block.prev_hash).or_default().insert(block.hash);
```

And on `remove_block`:
```rust
if let Some(successors) = self.successor_map.get_mut(&block.prev_hash) {
    successors.remove(&block.hash);
}
```

- [ ] **Step 2: Add `get_successors(&self, hash: &Hash32) -> &HashSet<Hash32>`**

- [ ] **Step 3: Use successor map for chain selection candidate enumeration**

- [ ] **Step 4: Commit**

```bash
git commit -m "feat(storage): add successor map to VolatileDB for O(1) fork enumeration"
```

---

### Task 6.4: Add ImmutableDB Validation on Startup

**Files:**
- Modify: `crates/torsten-storage/src/immutable_db.rs`

- [ ] **Step 1: Implement `validate_most_recent_chunk`**

Read the last chunk file, verify CRC32 of each block entry against the secondary index, truncate corrupt entries.

- [ ] **Step 2: Call on startup**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(storage): validate most recent ImmutableDB chunk on startup"
```

---

### Task 6.5: Reward Calculation — Use Exact Rational for expectedBlocks

**Files:**
- Modify: `crates/torsten-ledger/src/state/rewards.rs:262-265`

- [ ] **Step 1: Replace f64 arithmetic with Rat type**

```rust
// Before:
let one_minus_d = 1.0 - d;
let f = pp.active_slot_coeff();
let raw_expected_blocks = (one_minus_d * f * self.epoch_length as f64).floor() as u64;

// After:
let one_minus_d = Rat::new(1, 1) - Rat::from_f64(d);
let f = Rat::new(pp.active_slot_coeff_numerator, pp.active_slot_coeff_denominator);
let expected = (one_minus_d * f * Rat::new(self.epoch_length as i128, 1)).floor_to_u64();
```

- [ ] **Step 2: Commit**

```bash
git commit -m "fix(ledger): use exact rational arithmetic for expectedBlocks calculation"
```

---

### Task 6.6: DRep Inactivity — Account for Dormant Epochs

**Files:**
- Modify: `crates/torsten-ledger/src/state/epoch.rs:492`
- Modify: `crates/torsten-ledger/src/state/mod.rs` (add dormant epoch tracking)

**Context:** Haskell's `computeDRepExpiryVersioned` does not count dormant epochs (epochs with no active proposals) against DRep activity. Torsten counts all epochs.

- [ ] **Step 1: Track `num_dormant_epochs` in LedgerState**

A dormant epoch is one where no governance proposals were active. Increment at epoch boundary if `active_proposals.is_empty()`.

- [ ] **Step 2: Adjust DRep expiry calculation**

```rust
let active_epochs_elapsed = (new_epoch.0 - drep.last_active_epoch.0) - num_dormant_epochs_since(drep.last_active_epoch);
if active_epochs_elapsed > drep_activity {
    // Mark as inactive
}
```

- [ ] **Step 3: Commit**

```bash
git commit -m "fix(ledger): account for dormant epochs in DRep inactivity calculation"
```

---

### Task 6.7: WAL Compaction for VolatileDB

**Files:**
- Modify: `crates/torsten-storage/src/volatile_db.rs`

- [ ] **Step 1: Add `compact_wal()` method**

After blocks are flushed to ImmutableDB, rewrite the WAL with only the remaining volatile entries:
```rust
pub fn compact_wal(&mut self) -> Result<(), StorageError> {
    // Write all current blocks to a new WAL file
    let tmp_path = self.wal_path.with_extension("wal.tmp");
    let mut writer = WalWriter::new(&tmp_path)?;
    for block in self.blocks.values() {
        writer.append(block)?;
    }
    writer.flush()?;
    // Atomic rename
    std::fs::rename(&tmp_path, &self.wal_path)?;
    Ok(())
}
```

- [ ] **Step 2: Call `compact_wal` after `flush_to_immutable`**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(storage): compact VolatileDB WAL after flush to immutable"
```

---

### Task 6.8: Mithril Certificate Chain Verification

**Files:**
- Modify: `crates/torsten-node/src/mithril.rs`

**Context:** Currently only the digest is verified against the aggregator API. The STM multi-signature certificate chain is not verified, meaning a compromised aggregator could serve malicious snapshots.

- [ ] **Step 1: Research Mithril STM signature verification**

Determine whether a Rust crate for Mithril STM verification exists. If not, document the risk and add a warning log.

- [ ] **Step 2: Implement or document as known limitation**

- [ ] **Step 3: Commit**

---

## Verification Checklist

After completing all phases:

- [ ] `cargo build --all-targets` — zero errors
- [ ] `cargo test --all` — all tests pass
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt --all -- --check` — formatted
- [ ] Run node on preview testnet — syncs to tip
- [ ] Run node as relay — downstream Haskell peers sync correctly
- [ ] Submit a transaction via N2C — propagates to Haskell peers via TxSubmission2
- [ ] Forge a block as block producer — accepted by Haskell peers
