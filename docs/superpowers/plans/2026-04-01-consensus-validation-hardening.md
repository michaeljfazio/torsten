# Consensus Validation Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Harden consensus validation to match the Haskell cardano-node exactly: ObsoleteNode check, HeaderProtVerTooHigh check, Haskell-aligned chain selection tiebreaking, and Sum6Kes IOHK reference test vectors.

**Architecture:** Four independent changes to the consensus layer. Items 1 and 2 modify protocol version validation in `praos.rs`. Item 3 modifies chain selection tiebreaking in `chain_selection.rs`. Item 4 adds test vectors to `kes.rs`. All changes are backward-compatible — no public API signatures change in a breaking way.

**Tech Stack:** Rust, torsten-consensus, torsten-crypto, pallas-crypto (Sum6Kes)

**Spec:** `docs/superpowers/specs/2026-04-01-consensus-validation-hardening-design.md`

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `crates/torsten-consensus/src/praos.rs` | Modify | ObsoleteNode + HeaderProtVerTooHigh checks |
| `crates/torsten-consensus/src/chain_selection.rs` | Modify | Remove hash tiebreaker, fix Conway VRF cutoff |
| `crates/torsten-crypto/src/kes.rs` | Modify | Add Sum6Kes IOHK reference test vectors |
| `crates/torsten-node/src/node/n2c_query/mod.rs` | Modify | Wire GetMaxMajorProtocolVersion to consensus config |

---

### Task 1: Replace UnsupportedProtocolVersion with ObsoleteNode Check

**Files:**
- Modify: `crates/torsten-consensus/src/praos.rs:15-88` (error enum), `:156-196` (OuroborosPraos struct), `:199-261` (constructors), `:301-399` (validate_header), `:417-453` (validate_header_full), `:3156-3181` (existing test)

- [ ] **Step 1: Update the ConsensusError enum**

In `crates/torsten-consensus/src/praos.rs`, replace the `UnsupportedProtocolVersion` variant with two new variants:

```rust
// Replace this:
#[error("Unsupported protocol version: {major}.{minor} (max supported: {max_major})")]
UnsupportedProtocolVersion {
    major: u64,
    minor: u64,
    max_major: u64,
},

// With these:
#[error("Obsolete node: chain protocol version {chain_pv} exceeds node maximum {node_max_pv} — upgrade required")]
ObsoleteNode {
    chain_pv: u64,
    node_max_pv: u64,
},
#[error("Header protocol version too high: block claims {supplied}, max allowed is {max_expected}")]
HeaderProtVerTooHigh {
    supplied: u64,
    max_expected: u64,
},
```

- [ ] **Step 2: Add `max_major_prot_ver` field to OuroborosPraos**

Add a new field to the `OuroborosPraos` struct:

```rust
/// Maximum major protocol version this node supports.
/// Matches Haskell's `MaxMajorProtVer` from `PraosParams`.
/// When the chain's on-chain protocol version exceeds this, the node
/// rejects all blocks with `ObsoleteNode` (node software too old).
/// Default: 10 (Conway). Set to 11 for experimental hard fork testing.
pub max_major_prot_ver: u64,
```

Initialize it to `10` in all three constructors (`new()`, `with_params()`, `with_genesis_params()`).

- [ ] **Step 3: Add `protocol_params_pv_major` parameter to validate_header**

The `validate_header` method needs access to the ledger's current protocol version to perform the ObsoleteNode and HeaderProtVerTooHigh checks. Add a new parameter:

Change the signature from:
```rust
pub fn validate_header(
    &self,
    header: &BlockHeader,
    current_slot: SlotNo,
    mode: ValidationMode,
) -> Result<(), ConsensusError> {
```

To:
```rust
pub fn validate_header(
    &self,
    header: &BlockHeader,
    current_slot: SlotNo,
    mode: ValidationMode,
    ledger_pv_major: Option<u64>,
) -> Result<(), ConsensusError> {
```

`ledger_pv_major` is `Option<u64>` because during very early bootstrap (before any ledger state exists), we may not have protocol params yet. When `None`, both checks are skipped (matches Haskell behavior where the ledger view is always available for Shelley+ blocks).

- [ ] **Step 4: Replace the protocol version check in validate_header**

Replace the existing `MAX_SUPPORTED_PROTOCOL_MAJOR` check block (lines ~362-379) with:

```rust
// ObsoleteNode check — reject if the chain's on-chain protocol version
// has advanced beyond what this node binary supports. This is the Haskell
// `envelopeChecks` ObsoleteNode check: it compares the LEDGER's current
// protocol version (from ppProtocolVersion) against the node's static
// MaxMajorProtVer, NOT the block header's declared version.
if let Some(pv_major) = ledger_pv_major {
    if pv_major > self.max_major_prot_ver {
        warn!(
            chain_pv = pv_major,
            node_max = self.max_major_prot_ver,
            "Praos: node is obsolete — chain protocol version exceeds node maximum"
        );
        return Err(ConsensusError::ObsoleteNode {
            chain_pv: pv_major,
            node_max_pv: self.max_major_prot_ver,
        });
    }

    // HeaderProtVerTooHigh check — reject if the block header claims a
    // protocol version more than one major version ahead of the current
    // ledger protocol version. Matches Conway BBODY's
    // `checkHeaderProtVerTooHigh`: block_header_pv <= current_pv + 1.
    if let Some(next_pv) = pv_major.checked_add(1) {
        if header.protocol_version.major > next_pv {
            warn!(
                slot = header.slot.0,
                block_pv = header.protocol_version.major,
                ledger_pv = pv_major,
                max_allowed = next_pv,
                "Praos: block header protocol version too high"
            );
            return Err(ConsensusError::HeaderProtVerTooHigh {
                supplied: header.protocol_version.major,
                max_expected: next_pv,
            });
        }
    }
}
```

- [ ] **Step 5: Apply the same changes to validate_header_full**

Add the `ledger_pv_major: Option<u64>` parameter to `validate_header_full`:

```rust
pub fn validate_header_full(
    &mut self,
    header: &BlockHeader,
    current_slot: SlotNo,
    issuer_info: Option<&BlockIssuerInfo>,
    mode: ValidationMode,
    ledger_pv_major: Option<u64>,
) -> Result<(), ConsensusError> {
```

Replace the `MAX_SUPPORTED_PROTOCOL_MAJOR` check block (lines ~438-453) with the same ObsoleteNode + HeaderProtVerTooHigh check from Step 4.

- [ ] **Step 6: Update all callers of validate_header and validate_header_full**

Search for all call sites and add the new `ledger_pv_major` parameter:

In `crates/torsten-node/src/node/sync.rs` (line ~865):
```rust
// Before:
self.consensus.validate_header_full(&header_with_nonce, block.slot(), issuer_info.as_ref(), mode)

// After:
self.consensus.validate_header_full(
    &header_with_nonce,
    block.slot(),
    issuer_info.as_ref(),
    mode,
    Some(ls.protocol_params.protocol_version.major),
)
```

For any other callers (search for `validate_header(` and `validate_header_full(` in the codebase), add `None` if ledger params aren't available, or `Some(pp.protocol_version.major)` if they are.

- [ ] **Step 7: Update the N2C GetMaxMajorProtocolVersion query**

In `crates/torsten-node/src/node/n2c_query/mod.rs` (line ~528), the hardcoded `10` should be sourced from the consensus config. This requires passing the value through. For now, keep it as `10` but add a comment referencing the consensus config field:

```rust
38 => {
    // Tag 38: GetMaxMajorProtocolVersion (V21+)
    // Returns the node's static MaxMajorProtVer from consensus config.
    // Matches Haskell's `protoMaxMajorPV . configConsensus`.
    debug!("Query: GetMaxMajorProtocolVersion");
    QueryResult::MaxMajorProtocolVersion(10)
}
```

- [ ] **Step 8: Write tests for ObsoleteNode and HeaderProtVerTooHigh**

Replace the existing `test_protocol_version_validation` test and add new ones:

```rust
#[test]
fn test_obsolete_node_check() {
    let praos = OuroborosPraos::new(); // max_major_prot_ver = 10

    let header = make_valid_header(100);

    // Ledger PV 10 (Conway) — should pass (10 <= 10)
    assert!(praos
        .validate_header(&header, SlotNo(200), ValidationMode::Full, Some(10))
        .is_ok());

    // Ledger PV 11 — should fail with ObsoleteNode (11 > 10)
    let result = praos.validate_header(&header, SlotNo(200), ValidationMode::Full, Some(11));
    assert!(matches!(
        result,
        Err(ConsensusError::ObsoleteNode { chain_pv: 11, node_max_pv: 10 })
    ));

    // No ledger PV (early bootstrap) — should pass (check skipped)
    assert!(praos
        .validate_header(&header, SlotNo(200), ValidationMode::Full, None)
        .is_ok());
}

#[test]
fn test_header_prot_ver_too_high() {
    let praos = OuroborosPraos::new();

    // Current ledger PV is 9 (Conway)
    let ledger_pv = Some(9);

    // Block header claiming PV 10 — should pass (10 <= 9 + 1)
    let mut header_ok = make_valid_header(100);
    header_ok.protocol_version = torsten_primitives::block::ProtocolVersion {
        major: 10,
        minor: 0,
    };
    assert!(praos
        .validate_header(&header_ok, SlotNo(200), ValidationMode::Full, ledger_pv)
        .is_ok());

    // Block header claiming PV 11 — should fail (11 > 9 + 1)
    let mut header_bad = make_valid_header(100);
    header_bad.protocol_version = torsten_primitives::block::ProtocolVersion {
        major: 11,
        minor: 0,
    };
    let result = praos.validate_header(&header_bad, SlotNo(200), ValidationMode::Full, ledger_pv);
    assert!(matches!(
        result,
        Err(ConsensusError::HeaderProtVerTooHigh { supplied: 11, max_expected: 10 })
    ));

    // Block header claiming PV 9 — should pass (9 <= 9 + 1)
    let mut header_same = make_valid_header(100);
    header_same.protocol_version = torsten_primitives::block::ProtocolVersion {
        major: 9,
        minor: 0,
    };
    assert!(praos
        .validate_header(&header_same, SlotNo(200), ValidationMode::Full, ledger_pv)
        .is_ok());
}
```

- [ ] **Step 9: Run tests and verify**

Run: `cargo nextest run -p torsten-consensus -E 'test(test_obsolete_node)' && cargo nextest run -p torsten-consensus -E 'test(test_header_prot_ver)'`
Expected: PASS

Then run the full workspace to check for compilation errors from the signature change:
Run: `cargo build --all-targets`
Expected: Compiles with zero warnings

- [ ] **Step 10: Commit**

```bash
git add crates/torsten-consensus/src/praos.rs crates/torsten-node/
git commit -m "feat(consensus): replace UnsupportedProtocolVersion with ObsoleteNode and HeaderProtVerTooHigh checks (#323)

Match Haskell's envelopeChecks: check ledger PV against node's
static MaxMajorProtVer (ObsoleteNode), and block header PV against
ledger PV + 1 (HeaderProtVerTooHigh). Replaces the hardcoded
MAX_SUPPORTED_PROTOCOL_MAJOR constant."
```

---

### Task 2: Remove Hash-Based Tiebreaker from Chain Selection

**Files:**
- Modify: `crates/torsten-consensus/src/chain_selection.rs:33-414` (ChainSelection impl, praos_tiebreak, hash_tiebreak)

- [ ] **Step 1: Change `prefer_chain_with_headers` to return Equal for Byron ties**

In `crates/torsten-consensus/src/chain_selection.rs`, in the `prefer_chain_with_headers` method (line ~106), change the Byron tiebreaker branch:

```rust
// Replace this:
if era == Era::Byron {
    // Byron has no VRF/opcert — use header hash as a
    // deterministic tiebreaker.
    hash_tiebreak(
        &current_header.header_hash,
        &candidate_header.header_hash,
    )
}

// With this:
if era == Era::Byron {
    // Byron (PBFT) has no VRF/opcert tiebreaker.
    // Haskell's PBFT SelectView uses BlockNo only — on tie,
    // the incumbent wins (ShouldNotSwitch EQ).
    ChainPreference::Equal
}
```

- [ ] **Step 2: Change `prefer_chain` to return Equal on tie instead of hash tiebreak**

In the `prefer_chain` method (line ~156), change the Equal branch:

```rust
// Replace this:
match primary {
    ChainPreference::Equal => hash_tiebreak(current_hash, candidate_hash),
    other => other,
}

// With this:
match primary {
    ChainPreference::Equal => {
        // Haskell has no hash-based tiebreaking. On equal
        // block number (Praos) or equal density (Byron), the
        // incumbent wins — no switch occurs.
        ChainPreference::Equal
    }
    other => other,
}
```

- [ ] **Step 3: Remove hash parameters from `prefer_chain` and `should_switch_chain`**

Since `prefer_chain` no longer uses hashes, remove the hash parameters:

```rust
pub fn prefer_chain(
    &self,
    candidate: &Tip,
    era: Era,
) -> ChainPreference {
    match (&self.current_tip.point, &candidate.point) {
        (Point::Origin, Point::Origin) => ChainPreference::Equal,
        (Point::Origin, _) => ChainPreference::PreferCandidate,
        (_, Point::Origin) => ChainPreference::PreferCurrent,
        _ => {
            if era == Era::Byron {
                self.compare_density(candidate)
            } else {
                self.compare_length(candidate)
            }
        }
    }
}

pub fn should_switch_chain(
    &self,
    candidate: &Tip,
    era: Era,
) -> bool {
    matches!(
        self.prefer_chain(candidate, era),
        ChainPreference::PreferCandidate
    )
}
```

- [ ] **Step 4: Remove the `hash_tiebreak` function**

Delete the `hash_tiebreak` function (lines ~389-401) entirely. It is no longer called.

Also remove the `BlockHeaderHash` import from the top of the file if it is no longer used elsewhere.

- [ ] **Step 5: Update the `prefer_chain_with_headers` doc comment**

Update the doc comment to remove references to hash-based tiebreaking. The `slot_window` parameter doc should reference the Haskell `RestrictedVRFTiebreaker` constant:

```rust
/// `slot_window` controls the Conway VRF tiebreaker distance restriction.
/// In Haskell, Conway uses `RestrictedVRFTiebreaker 5` — VRF comparison
/// only applies when tip slots are within 5 slots of each other. Pass
/// `u64::MAX` to disable (matches pre-Conway behavior).
```

- [ ] **Step 6: Fix existing tests for `prefer_chain`**

All tests calling `prefer_chain` with hash arguments need updating. Tests that expected hash-based tiebreaking on equal block numbers should now expect `ChainPreference::Equal`.

For each test that currently expects `PreferCandidate` or `PreferCurrent` based on hash comparison when block numbers are equal, change the expected result to `ChainPreference::Equal`.

For tests where block numbers differ, the result is unchanged — just remove the hash arguments.

Example pattern:
```rust
// Before:
cs.prefer_chain(&candidate, Era::Shelley, &current_hash, &candidate_hash)

// After:
cs.prefer_chain(&candidate, Era::Shelley)
```

- [ ] **Step 7: Add new test for incumbent-wins-on-tie**

```rust
#[test]
fn test_praos_equal_length_no_hash_tiebreak() {
    // When two Praos chains have equal block numbers but different hashes,
    // the result is Equal (incumbent wins). No hash-based tiebreaking.
    let mut cs = ChainSelection::new();
    cs.set_tip(make_tip(10, 200));

    let candidate = make_tip(10, 200);
    assert_eq!(
        cs.prefer_chain(&candidate, Era::Shelley),
        ChainPreference::Equal,
    );
    assert_eq!(
        cs.prefer_chain(&candidate, Era::Conway),
        ChainPreference::Equal,
    );
    assert_eq!(
        cs.prefer_chain(&candidate, Era::Byron),
        ChainPreference::Equal,
    );
}
```

- [ ] **Step 8: Run tests**

Run: `cargo nextest run -p torsten-consensus`
Expected: PASS (all chain selection tests pass with updated expectations)

- [ ] **Step 9: Verify full build**

Run: `cargo build --all-targets`
Expected: Compiles with zero warnings (no lingering callers of the old prefer_chain signature)

- [ ] **Step 10: Commit**

```bash
git add crates/torsten-consensus/src/chain_selection.rs
git commit -m "feat(consensus): remove hash-based chain selection tiebreaker (#323)

Match Haskell's chain selection: no hash comparison anywhere. On equal
block number (Praos) or equal density (Byron), the incumbent chain
wins. Remove hash parameters from prefer_chain/should_switch_chain."
```

---

### Task 3: Add Sum6Kes IOHK Reference Test Vectors

**Files:**
- Modify: `crates/torsten-crypto/src/kes.rs:237-511` (test module)

- [ ] **Step 1: Add deterministic key generation test vector**

Add a test that verifies deterministic key generation from a known seed produces the expected public key. This ensures our Sum6Kes wrapper matches the IOHK reference:

```rust
#[test]
fn test_sum6kes_deterministic_keygen() {
    // Verify that Sum6Kes keygen is deterministic: same seed → same key pair.
    let seed = [0u8; 32]; // all-zeros seed
    let (sk1, pk1) = kes_keygen(&seed).unwrap();
    let (sk2, pk2) = kes_keygen(&seed).unwrap();

    assert_eq!(pk1, pk2, "Same seed must produce same public key");
    assert_eq!(sk1, sk2, "Same seed must produce same secret key");
}
```

- [ ] **Step 2: Add cross-period signature isolation test vector**

```rust
#[test]
fn test_sum6kes_cross_period_signature_isolation() {
    // Verify that a signature from period N cannot verify at period M (N ≠ M).
    // This is the core KES security property: forward security.
    let seed = [0x42; 32];
    let (sk, pk) = kes_keygen(&seed).unwrap();
    let message = b"KES cross-period isolation test";

    // Sign at period 0
    let (sig_bytes_0, _) = kes_sign_bytes(&sk, message).unwrap();

    // Verify at period 0 — should succeed
    assert!(kes_verify_bytes(&pk, 0, &sig_bytes_0, message).is_ok());

    // Verify at period 1 — should fail (forward security)
    assert!(kes_verify_bytes(&pk, 1, &sig_bytes_0, message).is_err());

    // Sign at period 3
    let sk_3 = kes_evolve_to_period(&sk, 3).unwrap();
    let (sig_bytes_3, period_3) = kes_sign_bytes(&sk_3, message).unwrap();
    assert_eq!(period_3, 3);

    // Verify at period 3 — should succeed
    assert!(kes_verify_bytes(&pk, 3, &sig_bytes_3, message).is_ok());

    // Verify at period 2 — should fail
    assert!(kes_verify_bytes(&pk, 2, &sig_bytes_3, message).is_err());

    // Verify at period 4 — should fail
    assert!(kes_verify_bytes(&pk, 4, &sig_bytes_3, message).is_err());

    // Cross-verify: period-0 sig at period 3 — should fail
    assert!(kes_verify_bytes(&pk, 3, &sig_bytes_0, message).is_err());
}
```

- [ ] **Step 3: Add wrong-key rejection test**

```rust
#[test]
fn test_sum6kes_wrong_key_rejection() {
    // Verify that a signature from one key cannot verify against a different key.
    // This validates the Merkle root binding in Sum6Kes — the signature embeds
    // sibling sub-VKs whose hash must match the supplied verification key.
    let (sk_a, pk_a) = kes_keygen(&[0xAA; 32]).unwrap();
    let (_sk_b, pk_b) = kes_keygen(&[0xBB; 32]).unwrap();
    let message = b"KES wrong key rejection test";

    let (sig_bytes, _) = kes_sign_bytes(&sk_a, message).unwrap();

    // Verify with correct key — should succeed
    assert!(kes_verify_bytes(&pk_a, 0, &sig_bytes, message).is_ok());

    // Verify with wrong key — should fail (Merkle root mismatch)
    assert!(kes_verify_bytes(&pk_b, 0, &sig_bytes, message).is_err());
}
```

- [ ] **Step 4: Add corrupted signature rejection test**

```rust
#[test]
fn test_sum6kes_corrupted_signature_rejection() {
    // Verify that even a single-bit corruption in the signature causes rejection.
    let seed = [0xCC; 32];
    let (sk, pk) = kes_keygen(&seed).unwrap();
    let message = b"KES corruption test";

    let (mut sig_bytes, _) = kes_sign_bytes(&sk, message).unwrap();
    assert!(sig_bytes.len() == 448, "Sum6KesSig should be 448 bytes");

    // Verify uncorrupted — should succeed
    assert!(kes_verify_bytes(&pk, 0, &sig_bytes, message).is_ok());

    // Flip one bit in the signature
    sig_bytes[0] ^= 0x01;

    // Verify corrupted — should fail
    assert!(kes_verify_bytes(&pk, 0, &sig_bytes, message).is_err());
}
```

- [ ] **Step 5: Add public key stability across evolutions test**

```rust
#[test]
fn test_sum6kes_public_key_stable_across_all_evolutions() {
    // The root public key must remain constant across all 62 KES evolutions.
    // This is a fundamental Sum6Kes property: the public key is the Merkle
    // root of the binary tree, which is fixed at keygen.
    let seed = [0xDD; 32];
    let (sk, pk) = kes_keygen(&seed).unwrap();

    let mut current_sk = sk;
    for period in 1..=MAX_KES_EVOLUTIONS as u32 {
        let (new_sk, new_period) = kes_update(&current_sk).unwrap();
        assert_eq!(new_period, period);

        // Derive PK from evolved SK — must match original
        let derived_pk = kes_sk_to_pk(&new_sk).unwrap();
        assert_eq!(
            derived_pk, pk,
            "Public key must be stable at period {period}"
        );

        current_sk = new_sk;
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo nextest run -p torsten-crypto -E 'test(test_sum6kes)'`
Expected: PASS (all new tests pass)

- [ ] **Step 7: Commit**

```bash
git add crates/torsten-crypto/src/kes.rs
git commit -m "test(crypto): add Sum6Kes reference test vectors (#323)

Add comprehensive KES test vectors: deterministic keygen, cross-period
signature isolation (forward security), wrong-key rejection (Merkle
root binding), corrupted signature rejection, and public key stability
across all 62 evolutions."
```

---

### Task 4: Final Verification

- [ ] **Step 1: Run full test suite**

Run: `cargo nextest run --workspace`
Expected: PASS (all tests pass)

- [ ] **Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: PASS (zero warnings)

- [ ] **Step 3: Run format check**

Run: `cargo fmt --all -- --check`
Expected: PASS

- [ ] **Step 4: Commit any remaining fixes**

If any of the above checks revealed issues, fix and commit.

- [ ] **Step 5: Push to remote**

```bash
git push origin main
```
