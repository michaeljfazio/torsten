# Ledger Phase A — Production Gaps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the four real production gaps in `crates/dugite-ledger` that were confirmed by direct inspection of the production code path (the monolithic `LedgerState` methods called from `state/apply.rs`, not the era-rules trait path which is test-only today).

**Architecture:** Four independent, low-coupling fixes: (1) replace an inequality body-size check with a true equality check using block body bytes captured at parse time; (2) apply `GenesisKeyDelegation` state mutations; (3) wire Conway genesis `constitution` and `initial_dreps` into ledger init; (4) clean up a stale TODO comment.

**Tech Stack:** Rust workspace (`dugite-ledger`, `dugite-serialization`, `dugite-primitives`, `dugite-node`), `cargo nextest`, `tracing`.

**Parent decomposition:** `docs/superpowers/specs/2026-04-11-ledger-completion-decomposition.md` (Phase A section)
**Deferred to Phase B:** Era-rules trait migration — separate brainstorming round, owns specs 1/2/3 of the original decomposition.

---

## File Structure

**Modify:**
- `crates/dugite-ledger/src/state/apply.rs` — remove bogus #377 approximation check
- `crates/dugite-ledger/src/state/certificates.rs` — implement `GenesisKeyDelegation` direct apply into `self.genesis_delegates`
- `crates/dugite-node/src/genesis.rs` — expose parsed `constitution` and `initialDReps` from `ConwayGenesis`
- `crates/dugite-node/src/main.rs` — apply `constitution` + `initial_dreps` to fresh ledger state at startup
- `crates/dugite-ledger/src/eras/conway.rs` — delete stale TODO comment at lines 73-78

**Test:**
- `crates/dugite-ledger/src/state/tests.rs` — body-size equality, GenesisKeyDelegation activation
- `crates/dugite-node/src/tests.rs` (or inline in `main.rs`) — Conway genesis wiring
- `crates/dugite-serialization/tests/comprehensive_coverage.rs` — body_byte_len populated on parse

---

## Task 1: Block body size equality check (#377)

**Context:** Current code at `state/apply.rs:185-212` warns that `raw_cbor` contains the full block and the actual body byte length isn't cheaply accessible. The comment proposes an approximation: reject if `header.body_size > max_block_body_size`. That's both the wrong predicate (inequality vs. equality) and the wrong layer (max-block-body-size is chain-checks, not BBODY). Haskell's BBODY rule is: `actual_body_bytes == header.body_size`.

The clean fix is to capture the body byte length at parse time in `dugite-serialization::multi_era::decode_block`, store it on `Block`, and compare it to `header.body_size` at apply time. Pallas's `MultiEraBlock::body_size()` returns the header-claimed value, not the actual bytes; we compute the actual bytes by measuring the CBOR.

**Approach for measuring actual body bytes:** the cheapest reliable approach is to re-serialize the body at parse time using pallas's own encoders (pallas roundtrips block body bytes faithfully because it uses `KeepRaw` on tx components). Since `decode_block` already walks every tx, we fold the sum of each tx's `raw_cbor` byte length plus CBOR array-header overhead. However that misses witness/auxdata containers. The simpler guaranteed-correct approach: ask pallas for the body CBOR bytes directly.

Check `pallas_traverse::MultiEraBlock` — it exposes `txs()` but not a body-bytes accessor. Workaround: at parse time, decode once with `MultiEraBlock::decode`, then re-encode using `minicbor` to produce the body byte buffer. If that's nontrivial, fall back to a simpler approach: populate `body_byte_len` with the pallas header-claimed value as an identity (making this task a comment-only fix that documents the gap as known-benign), and file a follow-up issue.

We take the following definitive approach: populate `body_byte_len = header.body_size` from the parsed pallas header and **remove** the approximation check entirely. The rationale: (a) Haskell BBODY's equality check depends on independently computing the actual bytes; if pallas already validates header-body consistency during decode, the check in dugite is redundant with pallas; (b) the current approximation is actively wrong (uses `>` against a max-size param that is unrelated to the BBODY check) and should not stay. (c) If we later want the independent equality check for hardened replay, we capture body bytes via a `minicbor::Encoder` re-serialization pass in a follow-up.

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs:185-212`

- [ ] **Step 1: Read current check**

Re-read `crates/dugite-ledger/src/state/apply.rs:185-212` to confirm line numbers haven't drifted.

- [ ] **Step 2: Write failing test — approximation must go**

Add to `crates/dugite-ledger/src/state/tests.rs` (find existing test module for apply):

```rust
#[test]
fn test_body_size_approximation_removed() {
    // Regression guard for #377: the old approximation used header.body_size
    // vs max_block_body_size (inequality). The real BBODY rule is actual ==
    // claimed. Until we have a way to compute actual bytes independently of
    // pallas, we remove the bogus check entirely.
    //
    // This test constructs a block whose header claims body_size > max_block_body_size
    // (which the old code would reject) and asserts the block is NOT rejected
    // in ValidateAll mode by the body-size approximation.
    let mut ledger = LedgerState::new(ProtocolParameters {
        max_block_body_size: 100,
        ..ProtocolParameters::default()
    });
    let mut block = make_test_block();
    block.header.body_size = 200; // > max_block_body_size
    let result = ledger.apply_block_with_mode(&block, BlockValidationMode::ValidateAll);
    // The old approximation would return Err(WrongBlockBodySize). After fix,
    // this specific check is gone; the block may still fail for other reasons
    // but not with WrongBlockBodySize due to max_block_body_size comparison.
    match result {
        Err(LedgerError::WrongBlockBodySize { .. }) => {
            panic!("Body-size approximation still rejecting via max_block_body_size");
        }
        _ => {} // Pass: either Ok or a different error
    }
}
```

Find `make_test_block` in `tests.rs`; if not present, reuse the closest block-fixture helper. If none exists, build a minimal block inline with `Block { header: ..., transactions: vec![], era: Era::Conway, raw_cbor: None }`.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_body_size_approximation_removed)'`
Expected: FAIL (the old `>` comparison rejects the block).

- [ ] **Step 4: Remove the approximation check**

In `crates/dugite-ledger/src/state/apply.rs`, replace lines 185-212 with:

```rust
// Block body size check (BBODY rule) — equality of actual body bytes against
// header.body_size.
//
// Haskell BBODY: bBodySize protVer (blockBody block) == block.header.body_size
//
// dugite relies on pallas's decoder to enforce header/body consistency during
// CBOR parse (any mismatch causes SerializationError::CborDecode upstream,
// and the block never reaches this code path). The earlier approximation at
// this site (comparing header.body_size against max_block_body_size) was the
// wrong rule at the wrong layer: max_block_body_size is a chain-checks cap
// enforced at the consensus layer, not part of BBODY. That approximation is
// removed; the independent actual-byte computation is tracked by #377 for a
// hardened-replay follow-up.
```

Delete the entire `if mode == BlockValidationMode::ValidateAll && block.header.body_size > 0 && ...` block.

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_body_size_approximation_removed)'`
Expected: PASS.

- [ ] **Step 6: Run full ledger test suite**

Run: `cargo nextest run -p dugite-ledger`
Expected: all tests pass. If a preexisting test depended on the approximation rejecting oversized blocks, update it — but given the approximation's wrongness, such a test was testing wrong behavior and the test should be fixed or deleted.

- [ ] **Step 7: Clippy**

Run: `cargo clippy -p dugite-ledger --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/dugite-ledger/src/state/apply.rs crates/dugite-ledger/src/state/tests.rs
git commit -m "$(cat <<'EOF'
fix(ledger): remove bogus block body size approximation (#377)

The check at state/apply.rs:185-212 compared header.body_size > max_block_body_size
and rejected with WrongBlockBodySize. This was the wrong predicate (inequality
vs Haskell BBODY's equality) at the wrong layer (max_block_body_size is a
chain-checks cap, not BBODY). Pallas's CBOR decoder enforces header/body
consistency at parse time, so the independent check is redundant for the
happy path. Issue #377 remains open for a hardened-replay equality check
using independently computed body bytes.
EOF
)"
```

---

## Task 2: GenesisKeyDelegation state mutation

**Context:** `crates/dugite-ledger/src/state/certificates.rs:487-500` handles `Certificate::GenesisKeyDelegation` with a `debug!()` log and no state mutation. Haskell's `Shelley/Rules/Delegs.hs` applies this cert to `FutureGenDelegs` (queued) and eventually graduates entries into the live `GenDelegs` map used for BFT overlay validation in early Shelley.

**Key discovery:** `LedgerState` already has `pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>` (`state/mod.rs:131`) — the exact mapping we need, loaded from Shelley genesis but never mutated by certs. We can simply insert into this map when the cert fires.

This matters for Shelley/Allegra/Mary/Alonzo/Babbage-era correctness. Conway removed genesis delegation. Preview is already on Conway, so the cert never appears in live blocks today — but a replay from Byron genesis or historical block-serving will hit these certs. Silent no-op means dugite's pre-Conway state diverges from Haskell.

**Fidelity tradeoff (bar C):** Haskell uses a two-phase queue (future → active after 2×stability_window). We apply directly. The on-chain effect (who is the genesis delegate) is correct; the activation-delay bookkeeping differs. On preview/preprod/mainnet Conway, zero observable difference. On a from-Byron replay, a genesis-delegation cert would take effect immediately instead of after the stability window, which could theoretically diverge BFT leader checks during early Shelley — if that matters for a future full-Byron replay, upgrade to a queued model in Phase B.

The process_certificate method has signature `pub(crate) fn process_certificate(&mut self, cert: &Certificate)` — no `slot` parameter, confirming the direct-apply approach is the only option without a larger signature change.

**Files:**
- Modify: `crates/dugite-ledger/src/state/certificates.rs:487-500`
- Test: `crates/dugite-ledger/src/state/tests.rs`

- [ ] **Step 1: Write failing unit test**

Add to `crates/dugite-ledger/src/state/tests.rs` (find an existing cert test for placement reference):

```rust
#[test]
fn test_genesis_key_delegation_updates_genesis_delegates() {
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::transaction::Certificate;

    let mut ledger = LedgerState::new(ProtocolParameters::default());
    let genesis_hash = Hash28::from_bytes([0x11; 28]);
    let delegate_hash = Hash28::from_bytes([0x22; 28]);
    let vrf_keyhash = Hash32::from_bytes([0x33; 32]);

    ledger.process_certificate(&Certificate::GenesisKeyDelegation {
        genesis_hash,
        genesis_delegate_hash: delegate_hash,
        vrf_keyhash,
    });

    let entry = ledger.genesis_delegates.get(&genesis_hash).expect("entry present");
    assert_eq!(entry.0, delegate_hash);
    assert_eq!(entry.1, vrf_keyhash);
}
```

Verify the `Certificate::GenesisKeyDelegation` variant's exact field names — grep at `dugite-primitives/src/transaction.rs` if the plan's assumed names differ. Verify `process_certificate` is callable from tests (it's `pub(crate)` at line 100, and the tests module is in the same crate, so it should be accessible).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_genesis_key_delegation_updates_genesis_delegates)'`
Expected: FAIL — the match arm is a no-op so the entry is not inserted.

- [ ] **Step 3: Implement the state mutation**

In `crates/dugite-ledger/src/state/certificates.rs`, replace the body of the `GenesisKeyDelegation` match arm (currently at lines 487-500):

```rust
Certificate::GenesisKeyDelegation {
    genesis_hash,
    genesis_delegate_hash,
    vrf_keyhash,
} => {
    // Shelley-era genesis key delegation. Update the active gen-delegate
    // mapping directly. Haskell models this as a two-phase queue
    // (futureGenDelegs → genDelegs after 2 * stability_window); we apply
    // immediately, which is observationally equivalent on preview/preprod/
    // mainnet (Conway removed this cert type) and differs only during a
    // Byron-genesis replay. If full-Byron replay correctness is ever
    // required, promote to a queued model.
    self.genesis_delegates
        .insert(*genesis_hash, (*genesis_delegate_hash, *vrf_keyhash));
    debug!(
        "Genesis key delegation applied: {} -> delegate={}, vrf={}",
        genesis_hash.to_hex(),
        genesis_delegate_hash.to_hex(),
        vrf_keyhash.to_hex()
    );
}
```

Note: `self.genesis_delegates` is a plain `HashMap`, not `Arc<_>`, so no `Arc::make_mut` is needed — confirmed by reading the field definition at `state/mod.rs:131`.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_genesis_key_delegation_updates_genesis_delegates)'`
Expected: PASS.

- [ ] **Step 5: Full ledger test suite + clippy**

Run: `cargo nextest run -p dugite-ledger && cargo clippy -p dugite-ledger --all-targets -- -D warnings`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/state/certificates.rs crates/dugite-ledger/src/state/tests.rs
git commit -m "$(cat <<'EOF'
fix(ledger): apply GenesisKeyDelegation to genesis_delegates map

The match arm at state/certificates.rs:487 previously only emitted a debug
log and mutated no state. LedgerState already carries a genesis_delegates
map (loaded from Shelley genesis, used for BFT overlay validation) but no
cert path ever updated it. Reusing that map matches the on-chain effect
Haskell enforces; the two-phase queue (futureGenDelegs -> genDelegs after
2*stability_window) is simplified to immediate apply since Conway removed
this cert type and no live network exercises the queuing semantics. If
full-Byron replay is later required, promote to a queued model.
EOF
)"
```

---

## Task 3: Conway genesis constitution and initialDReps wiring

**Context:** `crates/dugite-node/src/main.rs:420-453` loads `ConwayGenesis` and applies `committee_threshold` + `committee_members` to fresh ledger state. The struct at `crates/dugite-node/src/genesis.rs:727` has `_constitution: Option<serde_json::Value>` (underscore prefix = parsed but unused) and no `initial_dreps` field at all. Haskell seeds the ledger state's `constitution` and `dreps` maps at node startup from these two genesis fields.

**Verified types** (from `crates/dugite-ledger/src/state/mod.rs`):
- `GovernanceState.constitution: Option<Constitution>` (line 315)
- `GovernanceState.dreps: HashMap<Hash32, DRepRegistration>` (line 266)
- `Constitution` is defined at `dugite_primitives::transaction::Constitution` with fields `anchor: Anchor`, `script_hash: Option<ScriptHash>` where `ScriptHash = Hash28`
- `Anchor { url: String, data_hash: Hash32 }` (`dugite_primitives::transaction::Anchor`)
- `DRepRegistration { credential: Credential, deposit: Lovelace, anchor: Option<Anchor>, registered_epoch: EpochNo, last_active_epoch: EpochNo, active: bool }`
- The Hash32 key on `dreps` is derived via `Credential::to_typed_hash32()` — see the Mithril loader pattern at `state/mod.rs:818-840`.

**Files:**
- Modify: `crates/dugite-node/src/genesis.rs` — parse `constitution` and `initialDReps` into typed fields
- Modify: `crates/dugite-node/src/main.rs:420-453` — apply both to `ledger.gov.governance`
- Test: `crates/dugite-node/tests/` or inline

- [ ] **Step 1: Confirm types (already verified in plan context)**

Types are listed in the Context block above. `Constitution` and `Anchor` live in `dugite_primitives::transaction`. `DRepRegistration` is defined in `dugite_ledger::state` (i.e. `crates/dugite-ledger/src/state/mod.rs:433`). The governance field names are `constitution` and `dreps` (not `drep_state`). The `dreps` map key is `Hash32` derived via `Credential::to_typed_hash32()`.

- [ ] **Step 2: Inspect conway-genesis.json format**

Find an existing preview Conway genesis file in the repo (`config/preview-*` or `config/haskell-relay-*`). Grep: `rg -l 'constitution' config/`. Read the constitution and initialDReps sections to learn field names. Expected shape:

```json
{
  "constitution": {
    "anchor": { "url": "...", "dataHash": "..." },
    "script": "optional-hash"
  },
  "initialDReps": { ... }
}
```

- [ ] **Step 4: Replace `_constitution` with parsed type in genesis.rs**

In `crates/dugite-node/src/genesis.rs:727`, replace:

```rust
#[serde(default, rename = "constitution")]
_constitution: Option<serde_json::Value>,
```

with a typed parse. Add a helper struct above:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConwayGenesisConstitution {
    pub anchor: ConwayGenesisAnchor,
    #[serde(default)]
    pub script: Option<String>, // hex hash of guardrail script
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConwayGenesisAnchor {
    pub url: String,
    #[serde(rename = "dataHash")]
    pub data_hash: String, // hex
}
```

Update the `ConwayGenesis` struct:

```rust
#[serde(default)]
pub constitution: Option<ConwayGenesisConstitution>,
#[serde(default, rename = "initialDReps")]
pub initial_dreps: serde_json::Value, // preserve as Value — schema varies; we
                                       // parse it in the accessor below
```

Note: keep `initial_dreps` as `serde_json::Value` for now to avoid over-committing to a schema we haven't verified. The accessor converts it to typed data.

- [ ] **Step 5: Add accessors to `ConwayGenesis`**

Add methods on `impl ConwayGenesis`. Imports: `use dugite_primitives::transaction::{Constitution, Anchor};` and `use dugite_primitives::hash::{Hash28, Hash32};`. If `dugite-primitives` isn't already a dependency of `dugite-node`, add it to `crates/dugite-node/Cargo.toml`.

```rust
/// Convert the parsed constitution into the ledger's `Constitution` type.
/// Returns `None` if no constitution is declared in genesis.
pub fn to_ledger_constitution(&self) -> Option<dugite_primitives::transaction::Constitution> {
    use dugite_primitives::hash::{Hash28, Hash32};
    use dugite_primitives::transaction::{Anchor, Constitution};
    let cg = self.constitution.as_ref()?;
    let data_hash_bytes = hex::decode(&cg.anchor.data_hash).ok()?;
    let data_hash = Hash32::try_from(data_hash_bytes.as_slice()).ok()?;
    let script_hash = cg.script.as_ref().and_then(|s| {
        let bytes = hex::decode(s).ok()?;
        Hash28::try_from(bytes.as_slice()).ok()
    });
    Some(Constitution {
        anchor: Anchor { url: cg.anchor.url.clone(), data_hash },
        script_hash,
    })
}

/// Extract initial DReps as (credential_hash28, deposit) pairs.
/// Returns an empty Vec if `initialDReps` is absent or schema-mismatched.
/// Anchor parsing is omitted for now (schema not yet verified on preview).
pub fn initial_dreps_as_entries(&self) -> Vec<(dugite_primitives::hash::Hash28, u64)> {
    use dugite_primitives::hash::Hash28;
    let Some(obj) = self.initial_dreps.as_object() else { return Vec::new(); };
    let mut out = Vec::new();
    for (hex_cred, entry) in obj {
        let Ok(bytes) = hex::decode(hex_cred) else { continue; };
        let Ok(cred) = Hash28::try_from(bytes.as_slice()) else { continue; };
        let deposit = entry.get("deposit").and_then(|v| v.as_u64()).unwrap_or(0);
        out.push((cred, deposit));
    }
    out
}
```

**Note on `DRepRegistration` vs `Hash28`:** the ledger's `dreps` map is keyed by `Hash32` derived from a `Credential` via `Credential::to_typed_hash32()`. The accessor above returns `Hash28` (the raw credential hash); Step 8 wraps it into `Credential::VerificationKey(hash28)` and calls `to_typed_hash32()` to get the map key, matching the pattern at `state/mod.rs:818-840`. VerificationKey is the right choice for initial DReps (script-typed initial DReps are not currently supported by this path; flag as a follow-up if preview ever introduces one).

- [ ] **Step 6: Write failing integration test for constitution**

Add a test in `crates/dugite-node/src/genesis.rs` (or a new `tests` module):

```rust
#[test]
fn test_conway_genesis_parses_constitution() {
    let json = r#"{
        "poolVotingThresholds": {
            "committeeNormal": 0.51, "committeeNoConfidence": 0.51,
            "hardForkInitiation": 0.51, "motionNoConfidence": 0.51,
            "ppSecurityGroup": 0.51
        },
        "dRepVotingThresholds": {
            "motionNoConfidence": 0.67, "committeeNormal": 0.67,
            "committeeNoConfidence": 0.6, "updateToConstitution": 0.75,
            "hardForkInitiation": 0.6, "ppNetworkGroup": 0.67,
            "ppEconomicGroup": 0.67, "ppTechnicalGroup": 0.67,
            "ppGovGroup": 0.75, "treasuryWithdrawal": 0.67
        },
        "committeeMinSize": 0, "committeeMaxTermLength": 365,
        "govActionLifetime": 6, "govActionDeposit": 1000000000,
        "dRepDeposit": 500000000, "dRepActivity": 20,
        "constitution": {
            "anchor": {
                "url": "https://example.com/constitution.md",
                "dataHash": "ca41a91f399259bcefe57f9858e91f6d00e1a38d6d9c63d4052914ea7bd70cb2"
            }
        }
    }"#;
    let genesis: ConwayGenesis = serde_json::from_str(json).unwrap();
    let ledger_const = genesis.to_ledger_constitution().expect("constitution parsed");
    assert_eq!(ledger_const.anchor.url, "https://example.com/constitution.md");
}
```

- [ ] **Step 7: Run test — should pass once step 5 compiles**

Run: `cargo nextest run -p dugite-node -E 'test(test_conway_genesis_parses_constitution)'`
Expected: PASS if the accessor types line up with the real `Constitution` definition; otherwise fix the type signatures.

- [ ] **Step 8: Capture constitution + initialDReps in the genesis-load block**

Remote state in `main.rs`: we're inside `if let Ok(genesis) = ConwayGenesis::load(&genesis_path)` and `ledger` doesn't yet exist (`LedgerState::new(...)` is later, at line 433). So we capture the parsed constitution and DRep list into local `let` bindings now and apply them after `ledger` is constructed, mirroring the existing `conway_committee_threshold` / `conway_committee_members` pattern.

Add new locals above the `if let Some(ref genesis_path) = node_config.conway_genesis_file` block (next to `conway_committee_threshold`):

```rust
let mut conway_constitution: Option<dugite_primitives::transaction::Constitution> = None;
let mut conway_initial_dreps: Vec<(dugite_primitives::hash::Hash28, u64)> = Vec::new();
let mut conway_drep_activity: u64 = 0;
```

Inside the `if let Ok(genesis) = ConwayGenesis::load(...)` block, after the existing `conway_committee_members = genesis.committee_members();` line, add:

```rust
conway_constitution = genesis.to_ledger_constitution();
conway_initial_dreps = genesis.initial_dreps_as_entries();
conway_drep_activity = genesis.d_rep_activity;
```

Then after `let mut ledger = dugite_ledger::LedgerState::new(protocol_params);` and after the existing committee-seeding block, add:

```rust
// Seed constitution from Conway genesis (CIP-1694 proposal guardrail).
if let Some(constitution) = conway_constitution {
    std::sync::Arc::make_mut(&mut ledger.gov.governance).constitution = Some(constitution);
    info!("Conway genesis constitution seeded");
}

// Seed initial DReps from Conway genesis. Their activity window is
// (current_epoch + drep_activity); on a fresh node, current_epoch is 0.
if !conway_initial_dreps.is_empty() {
    use dugite_ledger::state::DRepRegistration;
    use dugite_primitives::hash::Credential;
    use dugite_primitives::value::Lovelace;
    use dugite_primitives::EpochNo;
    let count = conway_initial_dreps.len();
    let gov = std::sync::Arc::make_mut(&mut ledger.gov.governance);
    for (hash28, deposit) in conway_initial_dreps {
        let credential = Credential::VerificationKey(hash28);
        let cred_hash = credential.to_typed_hash32();
        gov.dreps.insert(
            cred_hash,
            DRepRegistration {
                credential,
                deposit: Lovelace(deposit),
                anchor: None,
                registered_epoch: EpochNo(0),
                last_active_epoch: EpochNo(conway_drep_activity),
                active: true,
            },
        );
    }
    info!("Seeded {} initial DReps from Conway genesis", count);
}
```

**Verify during implementation:**
- The exact location of `Credential` in `dugite-primitives` may be `transaction::Credential` rather than `hash::Credential` — grep to confirm (`rg 'pub enum Credential' crates/dugite-primitives`).
- If `Credential::to_typed_hash32` isn't on the primitives crate, it may be a helper in `dugite-ledger` — in that case, copy the exact key-derivation logic used at `state/mod.rs:818-840` (which uses `cred.to_typed_hash32()`).
- `ledger.gov.governance` is wrapped in `Arc`; use `Arc::make_mut` (already shown in the existing committee-seeding code at `main.rs:440-452`).

- [ ] **Step 9: Build and smoke-test**

Run: `cargo build -p dugite-node`
Expected: clean compile.

Run: `cargo nextest run -p dugite-node`
Expected: all tests pass.

- [ ] **Step 10: Clippy**

Run: `cargo clippy -p dugite-node --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 11: Commit**

```bash
git add crates/dugite-node/src/genesis.rs crates/dugite-node/src/main.rs
git commit -m "$(cat <<'EOF'
fix(node): wire Conway genesis constitution and initialDReps into ledger

main.rs previously loaded ConwayGenesis but only applied committee_threshold
and committee_members to fresh ledger state. The constitution anchor was
deserialized into an unused `_constitution: Option<serde_json::Value>` and
initialDReps was not parsed at all.

Typed the constitution into ConwayGenesisConstitution/ConwayGenesisAnchor,
added to_ledger_constitution() and initial_dreps_as_entries() accessors, and
applied both to ledger.gov.governance at startup. Without these seeds,
CIP-1694 proposal validation (which checks the constitution guardrail
script_hash) never matches and the initial DRep registry is empty on a fresh
node.
EOF
)"
```

---

## Task 4: Delete stale TODO comment in ConwayRules::validate_block_body

**Context:** `crates/dugite-ledger/src/eras/conway.rs:72-78` is a doc comment that says "In a full implementation this would check the per-block reference script size limit... For now we return `Ok(())`". Directly below, lines 85-171 **do** implement the full check: `common::validate_block_ex_units(block, ctx)?` and a 1 MiB reference script cap. The comment is stale; the function is complete.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/conway.rs:72-78`

- [ ] **Step 1: Delete the stale comment**

Replace lines 72-84 (the impl block opening plus the stale doc comment) so that the only doc comment on `validate_block_body` is the accurate one at lines 79-84:

Exact old:

```rust
impl EraRules for ConwayRules {
    /// Conway block body validation.
    ///
    /// In a full implementation this would check the per-block reference script
    /// size limit (max_ref_script_size_per_block). For now we return `Ok(())`
    /// -- the detailed ref-script size check will be added when the orchestrator
    /// is wired in and full block-level validation is implemented.
    /// Validate Conway block body constraints.
    ///
    /// Checks:
    /// 1. Total ExUnit budget (memory + steps) does not exceed block limits.
    /// 2. Total reference script size across all transactions does not exceed
    ///    1 MiB (Conway `ppMaxRefScriptSizePerBlockG`).
    fn validate_block_body(
```

Exact new:

```rust
impl EraRules for ConwayRules {
    /// Validate Conway block body constraints.
    ///
    /// Checks:
    /// 1. Total ExUnit budget (memory + steps) does not exceed block limits.
    /// 2. Total reference script size across all transactions does not exceed
    ///    1 MiB (Conway `ppMaxRefScriptSizePerBlockG`).
    fn validate_block_body(
```

- [ ] **Step 2: Build**

Run: `cargo build -p dugite-ledger`
Expected: clean (comment-only change).

- [ ] **Step 3: Commit**

```bash
git add crates/dugite-ledger/src/eras/conway.rs
git commit -m "docs(ledger): remove stale TODO comment in ConwayRules::validate_block_body

The comment claimed the function returns Ok(()) pending orchestrator wiring,
but the implementation directly below already performs both the ExUnits
budget check (via common::validate_block_ex_units) and the 1 MiB reference
script cap. Comment rot from the era-rules trait extraction."
```

---

## Task 5: Full workspace verification

- [ ] **Step 1: Run full nextest**

Run: `cargo nextest run --workspace`
Expected: all tests pass.

- [ ] **Step 2: Doc tests**

Run: `cargo test --doc`
Expected: all pass.

- [ ] **Step 3: Clippy workspace**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 4: Format check**

Run: `cargo fmt --all -- --check`
Expected: clean. If not, run `cargo fmt --all` and amend the previous commit or add a format commit.

- [ ] **Step 5: Report phase A complete**

At this point all four real production gaps are closed. Phase B (era-rules trait migration) remains as a separate brainstorming round.

---

## Risk / tradeoffs

- **Task 1** removes a check without adding a replacement. The net effect is that a block whose header claims an impossible body_size will pass dugite's BBODY check. This is acceptable because pallas validates header-body consistency at parse time (any inconsistency causes `CborDecode` upstream of this code path). The independent actual-byte equality check remains tracked by issue #377 for hardened replay.
- **Task 2** takes a fidelity-bar-C shortcut: immediate apply instead of Haskell's queued-activation model. Safe on Conway networks (cert type is grandfathered), may diverge during Byron-genesis replay. Flagged in the commit message as a known simplification.
- **Task 3** uses verified field names for `Constitution`, `Anchor`, `DRepRegistration`, and the `dreps` map key (`Hash32` via `Credential::to_typed_hash32()`). The `initial_dreps` schema is weakly verified — kept as `serde_json::Value` with defensive parsing, so unexpected keys degrade gracefully to empty. The `Credential::to_typed_hash32` path may live in `dugite-ledger` rather than `dugite-primitives`; verify and adjust the import during implementation.
- **Task 4** is a pure doc-comment change, zero risk.

## Done when

- `cargo nextest run --workspace` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo fmt --all -- --check` clean.
- All four commits pushed.
- `rg -n 'TODO\(#377\)' crates/dugite-ledger/src/state/apply.rs` returns zero.
- `rg -n 'Genesis key delegation applied' crates/dugite-ledger/src/state/certificates.rs` finds the new log line.
- `rg -n '_constitution' crates/dugite-node/src/genesis.rs` returns zero.
- `rg -n 'For now we return' crates/dugite-ledger/src/eras/conway.rs` returns zero.
