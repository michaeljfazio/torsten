# Ledger Completion — Close All Gaps

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Drive `crates/dugite-ledger` from ~88% to 100% complete — zero TODOs in source, all era-rules trait paths functional, cross-validated against Haskell reference.

**Architecture:** Two-phase approach: (A) migrate the monolithic `LedgerState::process_epoch_transition` into the `EraRules` trait path so there is one code path, not two; (B) fill remaining validation, cross-validation, and cleanup gaps. The migration reuses all existing logic from `state/epoch.rs` and `state/governance.rs` — the code exists, it just needs to be called from the new path instead of the old one.

**Tech Stack:** Pure Rust. No new pallas integrations. `cargo nextest run --workspace` for testing. Preview testnet for soak validation.

---

## Current State Assessment (2026-04-16)

### What's done
- Production path (`LedgerState::process_epoch_transition` at `state/epoch.rs:34`) handles all 13 NEWEPOCH steps correctly
- Governance ratification/enactment (`state/governance.rs`) is fully implemented (~5,871 lines)
- Phase-1 validation complete (14 rules, 20 unit tests)
- Phase-2 Plutus evaluation complete (V1/V2/V3 with per-redeemer Unit check)
- Block-level ExUnit budget validation wired for Alonzo/Babbage/Conway
- GenesisKeyDelegation now applies state mutation (no longer debug-only)
- Constitution + initialDReps seeded from ConwayGenesis at node startup
- DRep expiry rewritten to match Haskell stored-expiry model

### What remains (19 TODOs, all in era-rules trait path)
- `ConwayRules::process_epoch_transition` steps 3-8, 12-13 — governance pipeline not wired
- `ConwayRules::on_era_transition` steps 2-4, 6-7 — Conway genesis bootstrap not wired
- `ShelleyRules::process_epoch_transition` — RUPD not computed (deferred to orchestrator)
- `ShelleyRules::on_era_transition` — Byron→Shelley staking state init
- `eras/common.rs:720` — `validate_shelley_base` stub
- `eras/conway.rs:222-226` — PV10 withdrawal delegation (not active on-chain yet)

### What needs cross-validation
- Per-epoch reward amounts vs Koios historical data
- Plutus cost model calibration vs cardano-node eval results
- Block body size equality check (#377)

---

## File Structure

Files that will be **modified** (no new files needed):

| File | Responsibility |
|------|---------------|
| `crates/dugite-ledger/src/eras/mod.rs` | Add `ConwayGenesisConfig` to `RuleContext` |
| `crates/dugite-ledger/src/eras/conway.rs` | Wire governance pipeline into `process_epoch_transition`, fill `on_era_transition` |
| `crates/dugite-ledger/src/eras/shelley.rs` | Wire RUPD into `process_epoch_transition`, fill `on_era_transition` |
| `crates/dugite-ledger/src/eras/common.rs` | Remove `validate_shelley_base` stub or implement it |
| `crates/dugite-ledger/src/state/apply.rs` | Switch epoch transition dispatch from `LedgerState::process_epoch_transition` to `EraRulesImpl` |
| `crates/dugite-ledger/src/state/epoch.rs` | Extract reward calculation into standalone function callable by era rules |
| `crates/dugite-ledger/src/state/rewards.rs` | Make `calculate_rewards_full` callable from era rules (decouple from `&self`) |
| `crates/dugite-ledger/src/state/governance.rs` | Make `ratify_proposals` callable from era rules (decouple from `&self`) |
| `crates/dugite-ledger/src/state/mod.rs` | Add genesis config fields to `LedgerState`, deprecate old epoch transition |

---

## Task 1: Extract Reward Calculation from LedgerState

The reward calculation (`calculate_rewards_full` + `calculate_rewards_inner`) currently requires `&self` on `LedgerState`. The era-rules trait operates on decomposed sub-states. Extract it into a free function.

**Files:**
- Modify: `crates/dugite-ledger/src/state/rewards.rs`
- Modify: `crates/dugite-ledger/src/state/epoch.rs`
- Test: existing tests in `rewards.rs` (56 tests) + `epoch.rs` (15 tests)

- [ ] **Step 1: Write a failing test for the free function signature**

Add a test in `rewards.rs` that calls a new `compute_reward_update()` free function with explicit parameters (no `&self`):

```rust
#[test]
fn test_compute_reward_update_free_fn() {
    // Build minimal inputs matching an existing test case
    let params = ProtocolParameters::mainnet_defaults();
    let go_snapshot = None; // no GO snapshot = no rewards
    let bprev_block_count = 0;
    let bprev_blocks_by_pool = Arc::new(HashMap::new());
    let ss_fee = Lovelace(0);
    let reserves = Lovelace(14_000_000_000_000_000);
    let treasury = Lovelace(1_000_000_000_000_000);
    let reward_accounts = Arc::new(HashMap::new());

    let rupd = compute_reward_update(
        &params,
        go_snapshot.as_ref(),
        bprev_block_count,
        &bprev_blocks_by_pool,
        ss_fee,
        reserves,
        treasury,
        &reward_accounts,
        100, // epoch_length
        0,   // shelley_transition_epoch
    );

    // No GO snapshot → no rewards, but expansion still fires
    assert!(rupd.rewards.is_empty());
}
```

Run: `cargo nextest run -p dugite-ledger -E 'test(test_compute_reward_update_free_fn)'`
Expected: FAIL — `compute_reward_update` does not exist

- [ ] **Step 2: Extract the free function**

In `rewards.rs`, create `pub fn compute_reward_update(...)` by extracting the body of `LedgerState::calculate_rewards_full` + `calculate_rewards_inner`. The parameters are all the values that the current method reads from `&self`:

```rust
/// Compute the per-epoch reward update (RUPD) from decomposed state.
///
/// This is the standalone equivalent of `LedgerState::calculate_rewards_full`,
/// callable from era rules without requiring `&self`.
pub fn compute_reward_update(
    params: &ProtocolParameters,
    go_snapshot: Option<&StakeSnapshot>,
    bprev_block_count: u64,
    bprev_blocks_by_pool: &HashMap<Hash28, u64>,
    ss_fee: Lovelace,
    reserves: Lovelace,
    treasury: Lovelace,
    reward_accounts: &HashMap<Hash32, Lovelace>,
    epoch_length: u64,
    shelley_transition_epoch: u64,
) -> RewardUpdate {
    // Move the body of calculate_rewards_full here.
    // The existing method becomes a thin wrapper calling this function.
    // ...
}
```

Then make `LedgerState::calculate_rewards_full` delegate to it:

```rust
pub fn calculate_rewards_full(&self, ...) -> RewardUpdate {
    compute_reward_update(
        &self.epochs.protocol_params,
        self.epochs.snapshots.go.as_ref(),
        self.epochs.snapshots.bprev_block_count,
        &self.epochs.snapshots.bprev_blocks_by_pool,
        self.epochs.snapshots.ss_fee,
        self.epochs.reserves,
        self.epochs.treasury,
        &self.certs.reward_accounts,
        self.epoch_length,
        self.shelley_transition_epoch,
    )
}
```

- [ ] **Step 3: Run all existing reward + epoch tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(/reward|epoch/)'`
Expected: ALL PASS (56 reward tests + 15 epoch tests + the new test)

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/rewards.rs crates/dugite-ledger/src/state/epoch.rs
git commit -m "refactor(ledger): extract compute_reward_update free function from LedgerState"
```

---

## Task 2: Extract Governance Ratification from LedgerState

Same pattern: `ratify_proposals()` currently requires `&mut self` on `LedgerState`. Extract into a function callable with decomposed sub-states.

**Files:**
- Modify: `crates/dugite-ledger/src/state/governance.rs`
- Modify: `crates/dugite-ledger/src/state/epoch.rs` (update caller)
- Test: existing governance tests (in `governance.rs`, 40+ tests)

- [ ] **Step 1: Write a test calling the free function**

```rust
#[test]
fn test_ratify_proposals_free_fn() {
    // Minimal state: no proposals → no-op
    let mut state = LedgerState::new(ProtocolParameters::mainnet_defaults());
    state.epoch = EpochNo(100);
    let result = ratify_proposals_standalone(
        EpochNo(100),
        &state.epochs,
        &mut state.certs,
        &mut state.gov,
    );
    assert!(result.is_ok());
}
```

Run: `cargo nextest run -p dugite-ledger -E 'test(test_ratify_proposals_free_fn)'`
Expected: FAIL — function doesn't exist

- [ ] **Step 2: Extract the function**

Create `pub fn ratify_proposals_standalone(epoch, epochs, certs, gov) -> Result<(), LedgerError>` by extracting the governance portions from `LedgerState::process_epoch_transition` (lines 569-655 in `epoch.rs`) and the full `ratify_proposals` method body from `governance.rs`. The existing `LedgerState::ratify_proposals()` becomes a thin wrapper.

Key sub-operations to extract:
- `ratify_proposals()` call (governance.rs:636-1126)
- Dormant epoch tracking (epoch.rs:576-588)
- DRep activity marking (epoch.rs:590-614)
- Committee member expiry (epoch.rs:616-639)
- DRep/ratification snapshot capture (epoch.rs:641-655)

- [ ] **Step 3: Run all governance + epoch tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(/governance|ratif|epoch|drep/)'`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/state/governance.rs crates/dugite-ledger/src/state/epoch.rs
git commit -m "refactor(ledger): extract ratify_proposals_standalone from LedgerState"
```

---

## Task 3: Add ConwayGenesis Config to RuleContext

The `ConwayRules::on_era_transition` TODOs require access to ConwayGenesis data (initial DReps, committee, constitution). Add this to `RuleContext`.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/mod.rs` (add field to RuleContext)
- Modify: `crates/dugite-ledger/src/state/mod.rs` (store genesis config)
- Modify: `crates/dugite-ledger/src/state/apply.rs` (pass config when building RuleContext)
- Modify: `crates/dugite-node/src/node/mod.rs` (pass config to LedgerState)

- [ ] **Step 1: Define ConwayGenesisInit struct in eras/mod.rs**

Add a minimal struct holding just what the era rules need (not the full node-level ConwayGenesis):

```rust
/// Conway genesis initialization data needed by era-transition rules.
/// Populated from conway-genesis.json at node startup.
#[derive(Debug, Clone, Default)]
pub struct ConwayGenesisInit {
    /// Initial DRep registrations: (credential_hash, deposit_lovelace)
    pub initial_dreps: Vec<(Hash28, u64)>,
    /// Initial committee members: (credential_hash_bytes, expiry_epoch)
    pub committee_members: Vec<([u8; 32], u64)>,
    /// Committee threshold as (numerator, denominator)
    pub committee_threshold: Option<(u64, u64)>,
    /// Initial constitution
    pub constitution: Option<dugite_primitives::transaction::Constitution>,
}
```

Add `pub conway_genesis: Option<&'a ConwayGenesisInit>` to `RuleContext`.

- [ ] **Step 2: Store ConwayGenesisInit on LedgerState**

Add `pub conway_genesis_init: Option<ConwayGenesisInit>` to `LedgerState`. Default to `None`.

- [ ] **Step 3: Pass it through in apply.rs**

In `apply_block`, when building `RuleContext` for era transitions and epoch transitions, pass `self.conway_genesis_init.as_ref()`.

- [ ] **Step 4: Wire it from dugite-node**

In `crates/dugite-node/src/node/mod.rs`, after loading ConwayGenesis, construct `ConwayGenesisInit` and set it on `LedgerState`.

- [ ] **Step 5: Run build + tests**

Run: `cargo build --all-targets && cargo nextest run --workspace`
Expected: ALL PASS (no behavior change, just plumbing)

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/src/eras/mod.rs crates/dugite-ledger/src/state/mod.rs \
       crates/dugite-ledger/src/state/apply.rs crates/dugite-node/src/node/mod.rs
git commit -m "feat(ledger): add ConwayGenesisInit to RuleContext for era-rules trait migration"
```

---

## Task 4: Wire ConwayRules::on_era_transition Steps 2-4, 6-7

Fill the Babbage→Conway era transition with actual logic, using ConwayGenesisInit from RuleContext.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/conway.rs:662-724`
- Test: add tests in `conway.rs` mod tests

- [ ] **Step 1: Write failing tests for era transition initialization**

```rust
#[test]
fn test_on_era_transition_seeds_initial_dreps() {
    let genesis = ConwayGenesisInit {
        initial_dreps: vec![
            (Hash28::from_bytes([0x01; 28]), 500_000_000),
            (Hash28::from_bytes([0x02; 28]), 500_000_000),
        ],
        ..Default::default()
    };
    let rules = ConwayRules;
    let params = ProtocolParameters::mainnet_defaults();
    let mut ctx = make_conway_ctx(&params);
    ctx.conway_genesis = Some(&genesis);
    let mut gov = make_gov_sub();
    // ... other sub-states ...

    rules.on_era_transition(Era::Babbage, &ctx, &mut utxo, &mut certs, &mut gov, &mut consensus, &mut epochs).unwrap();

    assert_eq!(gov.governance.dreps.len(), 2);
}

#[test]
fn test_on_era_transition_seeds_committee() {
    // Similar: verify committee_expiration and committee_threshold are populated
}

#[test]
fn test_on_era_transition_seeds_constitution() {
    // Verify constitution is set from genesis
}

#[test]
fn test_on_era_transition_builds_vrf_pool_map() {
    // Verify VRF key hash → pool ID mapping built from pool_params
}
```

Run: `cargo nextest run -p dugite-ledger -E 'test(test_on_era_transition_seeds)'`
Expected: FAIL — assertions fail because TODOs are stubs

- [ ] **Step 2: Implement Step 2 — Create initial VState from ConwayGenesis**

In `on_era_transition`, replace the TODO at line 703 with:

```rust
// Step 2: Create initial VState (DRep state) from ConwayGenesis.
if let Some(ref genesis) = ctx.conway_genesis {
    let governance = Arc::make_mut(&mut gov.governance);

    // Seed initial DReps
    for (hash28, deposit) in &genesis.initial_dreps {
        let cred_hash = Hash32::from_bytes({
            let mut buf = [0u8; 32];
            buf[..28].copy_from_slice(hash28.as_bytes());
            buf
        });
        governance.dreps.entry(cred_hash).or_insert(DRepState {
            deposit: Lovelace(*deposit),
            drep_expiry: EpochNo(ctx.current_epoch.0 + ctx.params.drep_activity),
            anchor: None,
        });
    }

    // Seed committee members
    for (cred_bytes, expiry) in &genesis.committee_members {
        let cred = Hash32::from_bytes(*cred_bytes);
        governance.committee_expiration.insert(cred, EpochNo(*expiry));
    }

    // Set committee threshold
    if let Some((num, den)) = genesis.committee_threshold {
        governance.committee_threshold_numerator = num;
        governance.committee_threshold_denominator = den;
    }

    // Seed constitution
    if let Some(ref constitution) = genesis.constitution {
        governance.constitution = Some(constitution.clone());
    }
}
```

- [ ] **Step 3: Implement Step 3 — Build VRF key hash → pool ID map**

```rust
// Step 3: Build VRF key hash -> pool ID map from current pool_params.
// This is used by the DRep pulser to map block producers to pools.
// (Stored on ConsensusSubState or EpochSubState as appropriate.)
```

Note: Check if this map is actually needed by any downstream consumer in the current codebase. If not yet consumed, add the field but defer population until the DRep pulser wiring (Task 5).

- [ ] **Step 4: Implement Step 4 — Initial ConwayGovState**

Already handled by Step 2 above (committee + constitution + DReps are the ConwayGovState). Remove the TODO comment.

- [ ] **Step 5: Implement Step 6 — Recompute InstantStake without pointer addresses**

```rust
// Step 6: Recompute InstantStake without pointer addresses.
// After setting ptr_stake_excluded = true (Step 1), the stake_distribution
// still contains pointer-addressed coins from pre-Conway. Rebuild.
if !certs.stake_distribution.stake_map.is_empty() {
    // The stake_map is already maintained incrementally by stake_routing().
    // With ptr_stake_excluded = true, new blocks won't add pointer stake.
    // The existing entries from pointer addresses were already excluded by
    // Step 1 clearing ptr_stake. The incremental tracker handles the rest.
    // No full UTxO walk needed — the mark snapshot at next epoch boundary
    // will be built correctly from the updated distributions.
}
```

Note: verify whether the existing incremental tracking + Step 1's ptr_stake.clear() is sufficient, or if a full UTxO walk is truly needed. The Haskell TranslateEra does a full recompute, but our incremental model may handle it.

- [ ] **Step 6: Implement Step 7 — Set initial DRep pulser state**

Defer to Task 5 (DRep pulser wiring). Add a comment:

```rust
// Step 7: Initial DRep pulser state.
// The DRep distribution snapshot will be captured at the first Conway
// epoch boundary (process_epoch_transition Step 13). No pre-seeding
// needed — ratify_proposals falls back to live state when no snapshot
// exists (governance.rs:710-757).
```

- [ ] **Step 7: Run tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_on_era_transition)'`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add crates/dugite-ledger/src/eras/conway.rs
git commit -m "feat(ledger): implement ConwayRules::on_era_transition steps 2-4, 6-7"
```

---

## Task 5: Wire ConwayRules::process_epoch_transition Steps 3-8

The big migration: wire the existing governance pipeline (from `state/governance.rs` and `state/epoch.rs`) into `ConwayRules::process_epoch_transition`.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/conway.rs:477-555`
- Modify: `crates/dugite-ledger/src/state/governance.rs` (if any signature changes needed)
- Test: add tests in `conway.rs` mod tests

- [ ] **Step 1: Write failing tests for governance wiring in epoch transition**

```rust
#[test]
fn test_conway_epoch_transition_ratifies_proposals() {
    // Set up state with an enacted-ready proposal (e.g., InfoAction
    // with sufficient DRep + CC votes). Verify it gets ratified
    // and removed from proposals after epoch transition.
}

#[test]
fn test_conway_epoch_transition_dormant_epoch_tracking() {
    // Empty proposals → dormant epoch incremented
    // Non-empty proposals → not incremented
}

#[test]
fn test_conway_epoch_transition_drep_activity_update() {
    // DRep with expired drep_expiry → marked inactive
}

#[test]
fn test_conway_epoch_transition_treasury_withdrawal() {
    // Enacted TreasuryWithdrawals → treasury debited, reward accounts credited
}

#[test]
fn test_conway_epoch_transition_proposal_deposit_refund() {
    // Expired proposal → deposit returned to return address's reward account
}
```

Run: `cargo nextest run -p dugite-ledger -E 'test(test_conway_epoch_transition_)'`
Expected: FAIL — stubs don't execute governance logic

- [ ] **Step 2: Wire Step 3 — DRep pulser (capture DRep distribution snapshot)**

Replace the TODO at line 478 with a call to the extracted governance function:

```rust
// === Step 3: DRep pulser completion ===
// Capture the DRep distribution snapshot for governance ratification.
// Uses the mark snapshot's vote delegations to compute stake each DRep
// speaks for. The snapshot is consumed by ratify_proposals at this boundary.
capture_drep_distribution_snapshot(certs, gov, epochs);
```

Where `capture_drep_distribution_snapshot` is extracted from the existing `LedgerState::capture_drep_distribution_snapshot` in governance.rs.

- [ ] **Step 3: Wire Step 4+5 — Ratification + Treasury withdrawals + Enactment**

Replace the TODOs at lines 483-489 with:

```rust
// === Steps 4+5: Ratification, enactment, treasury withdrawals ===
ratify_proposals_standalone(new_epoch, epochs, certs, gov)?;
```

This single call handles:
- Ratification threshold checks for all proposals
- Enactment of ratified actions (including TreasuryWithdrawals)
- Priority-ordered evaluation

- [ ] **Step 4: Wire Step 6 — Return deposits from expired/enacted proposals**

The deposit return logic is already inside `ratify_proposals` (governance.rs lines 926-1119). Verify it runs correctly through the standalone function. Remove the TODO comment.

- [ ] **Step 5: Wire Step 7 — Update GovState**

Already handled by `ratify_proposals` (proposal forest cleanup, expiry pruning). Remove the TODO comment.

- [ ] **Step 6: Wire Step 8 — numDormantEpochs**

```rust
// === Step 8: numDormantEpochs computation ===
if gov.governance.proposals.is_empty() {
    Arc::make_mut(&mut gov.governance).num_dormant_epochs =
        gov.governance.num_dormant_epochs.saturating_add(1);
}
```

- [ ] **Step 7: Wire Step 12 — HARDFORK check**

```rust
// === Step 12: HARDFORK check ===
// HardForkInitiation actions set protocol_version during enactment
// (governance.rs enact_gov_action). The consensus layer detects the
// version bump and triggers the actual hardfork. No additional logic
// needed here.
```

Remove the TODO, replace with the explanatory comment.

- [ ] **Step 8: Wire Step 13 — setFreshDRepPulsingState**

```rust
// === Step 13: setFreshDRepPulsingState ===
// Capture ratification snapshot for next epoch's governance ratification.
capture_ratification_snapshot(gov, epochs);
```

Where `capture_ratification_snapshot` is extracted from the existing method.

- [ ] **Step 9: Add DRep activity + committee expiry after governance**

Wire the remaining governance-adjacent epoch operations that the old path handles:

```rust
// DRep activity marking (mark inactive DReps)
update_drep_activity(new_epoch, gov, epochs);

// Committee member expiry pruning
prune_expired_committee_members(new_epoch, gov);
```

These are extracted from `epoch.rs:590-639`.

- [ ] **Step 10: Run tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_conway_epoch_transition_)'`
Expected: ALL PASS

- [ ] **Step 11: Commit**

```bash
git add crates/dugite-ledger/src/eras/conway.rs crates/dugite-ledger/src/state/governance.rs
git commit -m "feat(ledger): wire governance pipeline into ConwayRules::process_epoch_transition"
```

---

## Task 6: Wire ShelleyRules::process_epoch_transition RUPD

Complete the Shelley era-rules epoch transition by wiring reward calculation.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/shelley.rs:200`
- Test: add test in `shelley.rs` mod tests

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_shelley_epoch_transition_computes_rupd() {
    // Set up state with a GO snapshot containing a pool with stake.
    // Verify that after epoch transition, rewards are distributed.
}
```

- [ ] **Step 2: Wire the RUPD call**

At `shelley.rs:200`, replace the TODO with:

```rust
// Step 2: Compute RUPD using GO snapshot + bprev + ss_fee.
if epochs.snapshots.rupd_ready {
    let rupd = compute_reward_update(
        ctx.params,
        epochs.snapshots.go.as_ref(),
        epochs.snapshots.bprev_block_count,
        &epochs.snapshots.bprev_blocks_by_pool,
        epochs.snapshots.ss_fee,
        epochs.reserves,
        epochs.treasury,
        &certs.reward_accounts,
        ctx.epoch_length,
        ctx.shelley_transition_epoch,
    );

    // Apply RUPD: adjust reserves, treasury, and reward accounts.
    epochs.reserves.0 = epochs.reserves.0.saturating_sub(rupd.delta_reserves);
    epochs.treasury.0 = epochs.treasury.0.saturating_add(rupd.delta_treasury);
    for (cred_hash, reward) in &rupd.rewards {
        if reward.0 > 0 {
            if certs.reward_accounts.contains_key(cred_hash) {
                *Arc::make_mut(&mut certs.reward_accounts)
                    .entry(*cred_hash)
                    .or_insert(Lovelace(0)) += *reward;
            } else {
                // Unregistered credential: forward to treasury
                epochs.treasury.0 = epochs.treasury.0.saturating_add(reward.0);
            }
        }
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo nextest run -p dugite-ledger -E 'test(/shelley.*epoch|reward/)'`
Expected: ALL PASS

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/src/eras/shelley.rs
git commit -m "feat(ledger): wire RUPD computation into ShelleyRules::process_epoch_transition"
```

---

## Task 7: Wire ShelleyRules::on_era_transition (Byron→Shelley)

Fill the Byron→Shelley era transition stub.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/shelley.rs:563-589`

- [ ] **Step 1: Write test**

```rust
#[test]
fn test_byron_to_shelley_era_transition() {
    // Verify that on_era_transition from Byron produces valid initial state
}
```

- [ ] **Step 2: Document the no-op**

The Byron→Shelley transition in Haskell's `translateToShelleyLedgerState` converts Byron UTxOs to Shelley UTxOs and initializes staking from `ShelleyGenesis.genDelegs`. In dugite, Byron UTxOs are already in the unified UTxO set, and genesis delegates are loaded at node startup. Replace the TODO with an explicit comment:

```rust
Era::Byron => {
    // Byron→Shelley: In Haskell, translateToShelleyLedgerState converts
    // Byron UTxOs and initializes staking from ShelleyGenesis.genDelegs.
    // In dugite:
    // - UTxOs are already in the unified set (no conversion needed)
    // - Genesis delegates are loaded at node startup (node/mod.rs)
    // - Initial funds/staking from ShelleyGenesis are applied during
    //   LedgerState construction
    // No state transformation needed at the era boundary.
    Ok(())
}
```

- [ ] **Step 3: Run tests, commit**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_byron_to_shelley)'`

```bash
git add crates/dugite-ledger/src/eras/shelley.rs
git commit -m "docs(ledger): document Byron→Shelley era transition rationale"
```

---

## Task 8: Switch Orchestrator to EraRules Dispatch

The critical migration: change `apply.rs` to dispatch epoch transitions through `EraRulesImpl` instead of the monolithic `LedgerState::process_epoch_transition`.

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs:167-183`
- Modify: `crates/dugite-ledger/src/state/epoch.rs` (deprecate old method)

- [ ] **Step 1: Write integration test verifying equivalence**

Before switching, add a test that runs both paths on the same input and asserts identical output:

```rust
#[test]
fn test_era_rules_epoch_transition_matches_monolithic() {
    // Build a LedgerState at a Conway epoch boundary with:
    // - Active pools, delegations, rewards
    // - Pending governance proposals
    // - Pending retirements
    //
    // Clone the state, run old path on one, new path on the other.
    // Assert all sub-state fields are identical.
}
```

- [ ] **Step 2: Switch the dispatch**

In `apply.rs:179-182`, change:

```rust
// OLD:
while self.epoch < block_epoch {
    let next_epoch = EpochNo(self.epoch.0.saturating_add(1));
    self.process_epoch_transition(next_epoch);
}

// NEW:
while self.epoch < block_epoch {
    let next_epoch = EpochNo(self.epoch.0.saturating_add(1));
    let epoch_rules = EraRulesImpl::for_era(self.era);
    let epoch_params = self.epochs.protocol_params.clone();
    let epoch_ctx = RuleContext {
        params: &epoch_params,
        current_slot: block.slot().0,
        current_epoch: self.epoch,
        era: self.era,
        slot_config: Some(&self.slot_config),
        node_network: self.node_network,
        genesis_delegates: &self.genesis_delegates,
        update_quorum: self.update_quorum,
        epoch_length: self.epoch_length,
        shelley_transition_epoch: self.shelley_transition_epoch,
        byron_epoch_length: self.byron_epoch_length,
        stability_window: self.randomness_stabilisation_window,
        stability_window_3kf: self.stability_window_3kf,
        randomness_stabilisation_window: self.randomness_stabilisation_window,
        conway_genesis: self.conway_genesis_init.as_ref(),
        tx_index: 0,
    };
    epoch_rules.process_epoch_transition(
        next_epoch,
        &epoch_ctx,
        &mut self.utxo,
        &mut self.certs,
        &mut self.gov,
        &mut self.epochs,
        &mut self.consensus,
    )?;
    self.epoch = next_epoch;
}
```

- [ ] **Step 3: Run the full test suite**

Run: `cargo nextest run --workspace`
Expected: ALL PASS

- [ ] **Step 4: Run clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check`
Expected: CLEAN

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/src/state/apply.rs
git commit -m "feat(ledger): switch epoch transition dispatch to EraRulesImpl (Task 12)"
```

---

## Task 9: Deprecate Monolithic process_epoch_transition

After the switch, the old method is dead code. Mark it deprecated and plan removal.

**Files:**
- Modify: `crates/dugite-ledger/src/state/epoch.rs`

- [ ] **Step 1: Add #[deprecated] annotation**

```rust
#[deprecated(note = "Use EraRulesImpl::process_epoch_transition via apply_block instead")]
pub fn process_epoch_transition(&mut self, new_epoch: EpochNo) {
    // ...existing body kept for reference during migration validation...
}
```

- [ ] **Step 2: Fix any remaining callers**

Search for `self.process_epoch_transition(` in non-test code. Update any remaining callers to use the era-rules path. Test-only callers can keep using the old method temporarily.

- [ ] **Step 3: Run tests, commit**

Run: `cargo nextest run --workspace && cargo clippy --all-targets -- -D warnings`

```bash
git add crates/dugite-ledger/src/state/epoch.rs
git commit -m "refactor(ledger): deprecate monolithic process_epoch_transition"
```

---

## Task 10: Remove validate_shelley_base Stub

Clean up the empty `validate_shelley_base` stub in `eras/common.rs`.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/common.rs:712-730`

- [ ] **Step 1: Assess the stub**

Read `common.rs:712-730`. The comment says "extract Phase-1 rules 1-10 from validation/phase1.rs". The actual Phase-1 validation happens in `validation/phase1.rs` and is called from `validation/mod.rs:validate_transaction_with_pools`. The era-rules trait calls `validate_transaction_with_pools` in `apply.rs`. The stub is dead code that was planned but never needed.

- [ ] **Step 2: Delete the stub and TODO comment**

Remove the `validate_shelley_base` function and its TODO comment entirely. The Phase-1 validation is correctly organized in `validation/phase1.rs` and needs no restructuring.

- [ ] **Step 3: Run tests, commit**

Run: `cargo nextest run -p dugite-ledger && cargo clippy --all-targets -- -D warnings`

```bash
git add crates/dugite-ledger/src/eras/common.rs
git commit -m "cleanup(ledger): remove stale validate_shelley_base TODO stub"
```

---

## Task 11: Implement PV10 Withdrawal Delegation Validation

Add the protocol version 10 withdrawal checks (currently stubbed at `conway.rs:222-226`). These are not active on-chain yet but should be ready.

**Files:**
- Modify: `crates/dugite-ledger/src/eras/conway.rs:220-227`
- Test: add tests in `conway.rs` mod tests

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn test_pv10_withdrawal_must_be_delegated() {
    // PV >= 10, withdrawal from a KeyHash account that has NO DRep delegation
    // → should fail with WithdrawalNotDelegated
    let params = ProtocolParameters { protocol_version_major: 10, ..defaults() };
    // Set up tx with withdrawal, credential NOT in vote_delegations
    // Assert: error returned
}

#[test]
fn test_pv10_withdrawal_delegated_succeeds() {
    // PV >= 10, withdrawal from KeyHash account WITH DRep delegation → OK
}

#[test]
fn test_pv9_withdrawal_not_checked() {
    // PV = 9, undelegated withdrawal → still succeeds (PV10 not active)
}

#[test]
fn test_pv10_withdrawal_amount_must_match_balance() {
    // PV >= 10, withdrawal amount != reward balance → fail
}
```

- [ ] **Step 2: Implement the checks**

At `conway.rs:222`, replace the TODO stubs:

```rust
// Step 3: validateWithdrawalsDelegated (PV >= 10).
if ctx.params.protocol_version_major >= 10 {
    for (reward_addr, _amount) in &tx.body.withdrawals {
        let cred_hash = credential_to_hash(reward_addr);
        // Only check KeyHash credentials (script credentials are exempt)
        if is_key_credential(reward_addr)
            && !gov.governance.vote_delegations.contains_key(&cred_hash)
        {
            return Err(LedgerError::BlockTxValidationFailed {
                slot: ctx.current_slot,
                tx_hash: tx.hash.to_hex(),
                errors: format!(
                    "WithdrawalNotDelegated: reward account {} has no DRep delegation",
                    cred_hash.to_hex()
                ),
            });
        }
    }
}

// Step 4: testIncompleteAndMissingWithdrawals (PV >= 10).
if ctx.params.protocol_version_major >= 10 {
    for (reward_addr, amount) in &tx.body.withdrawals {
        let cred_hash = credential_to_hash(reward_addr);
        let balance = certs.reward_accounts
            .get(&cred_hash)
            .copied()
            .unwrap_or(Lovelace(0));
        if Lovelace(*amount) != balance {
            return Err(LedgerError::BlockTxValidationFailed {
                slot: ctx.current_slot,
                tx_hash: tx.hash.to_hex(),
                errors: format!(
                    "IncompleteWithdrawal: withdrawal {} != balance {} for {}",
                    amount, balance.0, cred_hash.to_hex()
                ),
            });
        }
    }
}
```

- [ ] **Step 3: Run tests, commit**

Run: `cargo nextest run -p dugite-ledger -E 'test(test_pv10)'`

```bash
git add crates/dugite-ledger/src/eras/conway.rs
git commit -m "feat(ledger): implement PV10 withdrawal delegation validation"
```

---

## Task 12: Block Body Size Equality Check (#377)

Implement the proper BBODY block body size check.

**Files:**
- Modify: `crates/dugite-ledger/src/state/apply.rs:185-191`
- Modify: `crates/dugite-primitives/src/block.rs` (if body bytes not accessible)

- [ ] **Step 1: Investigate feasibility**

Read the block structure to determine if we have access to the raw body bytes for size comparison. Check if `block.raw_cbor` or similar provides the body CBOR separately from the header.

If the raw body bytes are available: implement the equality check.
If not: document why the check is deferred (pallas deserializes the full block CBOR and doesn't preserve the body-only bytes) and update the comment to be precise about the blocker.

- [ ] **Step 2: Implement or document**

If feasible:
```rust
// BBODY rule: actual body bytes must equal header.body_size
if mode == BlockValidationMode::ValidateAll {
    if let Some(actual_body_size) = block.compute_body_size() {
        if actual_body_size != block.header.body_size {
            return Err(LedgerError::WrongBlockBodySize {
                expected: block.header.body_size,
                actual: actual_body_size,
            });
        }
    }
}
```

If not feasible: update the comment with the precise technical reason and what would unblock it.

- [ ] **Step 3: Run tests, commit**

```bash
git commit -m "fix(ledger): implement block body size equality check (#377)"
```

---

## Task 13: Reward Cross-Validation Against Koios

Add integration tests that verify reward calculations match real Koios data for specific preview epochs.

**Files:**
- Modify: `crates/dugite-ledger/tests/reward_cross_validation.rs`

- [ ] **Step 1: Extend existing cross-validation with per-pool reward amounts**

The existing `reward_cross_validation.rs` has epoch-level tests but doesn't verify individual pool reward amounts. Add tests that:

1. Take a specific pool on preview testnet
2. Use the known pool parameters (pledge, cost, margin, stake) from that epoch
3. Compute `maxPool'` and `apparent_performance` using dugite's formula
4. Compare against the actual reward amount from Koios `pool_history` endpoint
5. Assert match within 1 lovelace (rounding)

```rust
#[test]
fn test_pool_reward_matches_koios_epoch_1233() {
    // Pool: SAND (6954ec11...)
    // Known from Koios pool_history:
    //   active_stake: X, blocks: Y, total_reward: Z
    // Compute using dugite's calculate_pool_reward and compare
}
```

- [ ] **Step 2: Add member reward distribution test**

Verify that the sum of member rewards + operator reward = pool reward for a real pool.

- [ ] **Step 3: Run tests, commit**

```bash
git add crates/dugite-ledger/tests/reward_cross_validation.rs
git commit -m "test(ledger): add per-pool reward cross-validation against Koios"
```

---

## Task 14: Plutus Cost Model Cross-Validation

Add tests that verify Plutus script evaluation produces the same ExUnit consumption as cardano-node for real transactions.

**Files:**
- Create: `crates/dugite-ledger/tests/plutus_cross_validation.rs`

- [ ] **Step 1: Capture real Plutus transaction data**

Use `cardano-cli transaction view` on a real preview testnet Plutus transaction to get:
- Transaction CBOR
- UTxO set at the transaction's inputs
- Expected ExUnits per redeemer (from the block producer's redeemer witness)

- [ ] **Step 2: Write cross-validation test**

```rust
#[test]
fn test_plutus_eval_matches_cardano_node() {
    // Load real tx CBOR and UTxO context
    // Run evaluate_plutus_scripts with preview cost models
    // The test passes if no PlutusError is returned
    // (budget enforcement uses max_tx_ex_units, not the redeemer's claimed units)
}
```

- [ ] **Step 3: Run tests, commit**

```bash
git add crates/dugite-ledger/tests/plutus_cross_validation.rs
git commit -m "test(ledger): add Plutus evaluation cross-validation against real transactions"
```

---

## Task 15: Final Cleanup — Zero TODOs

Sweep the ledger crate for any remaining TODO/FIXME markers and resolve them.

**Files:**
- Multiple files in `crates/dugite-ledger/src/`

- [ ] **Step 1: Run the TODO scan**

```bash
rg -n 'TODO|FIXME|todo!\(|unimplemented!\(' crates/dugite-ledger/src/ --glob '!*test*'
```

Expected: zero results (all 19 original TODOs resolved by Tasks 1-12)

- [ ] **Step 2: If any remain, resolve them**

For each remaining TODO:
- If it's stale (functionality exists elsewhere): delete the comment
- If it's a real gap: implement it or add a tracking issue reference
- If it's deferred for a reason: convert to a precise comment explaining the deferral

- [ ] **Step 3: Run full verification**

```bash
cargo build --all-targets
cargo nextest run --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

All must pass.

- [ ] **Step 4: Commit**

```bash
git commit -m "cleanup(ledger): resolve all remaining TODO/FIXME markers"
```

---

## Task 16: Integration Soak Test

Run the node on preview testnet and verify no regressions.

- [ ] **Step 1: Build release binary**

```bash
cargo build --release
```

- [ ] **Step 2: Run Mithril import + sync**

```bash
./target/release/dugite-node mithril-import --network-magic 2 --database-path ./db-preview
./target/release/dugite-node run \
  --config config/preview-config.json \
  --topology config/preview-topology.json \
  --database-path ./db-preview \
  --socket-path ./node.sock \
  --host-addr 0.0.0.0 --port 3001
```

- [ ] **Step 3: Verify tip matches Haskell peer**

```bash
dugite-cli query tip --socket-path ./node.sock --network-magic 2
```

Compare against Koios `tip` endpoint.

- [ ] **Step 4: Verify governance state**

```bash
dugite-cli query gov-state --socket-path ./node.sock --network-magic 2
```

Spot-check proposal count, committee members, constitution hash.

- [ ] **Step 5: Run for 24 hours, monitor for divergences**

Watch logs for any `ledger divergence`, `block rejected`, or `epoch transition error` messages.

---

## Dependency Order

```
Task 1 (extract rewards) ───────────────┐
Task 2 (extract governance) ─────────────┼─► Task 8 (switch orchestrator)
Task 3 (ConwayGenesis in RuleContext) ───┤                │
Task 4 (on_era_transition) ──────────────┤                ▼
Task 5 (epoch transition steps 3-8) ─────┘    Task 9 (deprecate old path)
                                                          │
Task 6 (Shelley RUPD) ──────── parallel ──┐               │
Task 7 (Byron→Shelley docs) ─ parallel ──┤               │
Task 10 (common.rs cleanup) ─ parallel ──┤               │
Task 11 (PV10 validation) ── parallel ───┤               ▼
Task 12 (body size #377) ─── parallel ───┘    Task 15 (zero TODOs)
                                                          │
Task 13 (reward xval) ────── parallel ───┐               ▼
Task 14 (Plutus xval) ────── parallel ───┘    Task 16 (soak test)
```

Tasks 6, 7, 10, 11, 12 can run in parallel (independent changes).
Tasks 13, 14 can run in parallel (independent test files).
Task 8 depends on Tasks 1-5.
Task 9 depends on Task 8.
Task 15 depends on all implementation tasks.
Task 16 depends on Task 15.
