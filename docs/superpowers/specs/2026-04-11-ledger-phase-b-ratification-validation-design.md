# Phase B — Conway Ratification Validation

**Date:** 2026-04-11
**Parent:** [Ledger Completion Decomposition](2026-04-11-ledger-completion-decomposition.md) — Phase B (retargeted)
**Depends on:** Phase A (merged)

---

## Goal

Validate dugite's `LedgerState::ratify_proposals()` produces the same enacted set as Haskell cardano-node for ratified governance proposals on preview testnet. The first Phase B sub-project is a narrow, validation-driven slice: capture a small number of real preview proposals that reached ratification, replay the ratify algorithm against them with Koios-derived state, assert outcomes match.

## Context

Phase A closed the four remaining documented TODOs in the production ledger path and revealed an important reframing: the production code has zero TODO/FIXME markers in epoch, governance, rewards, and validation modules. Any remaining fidelity gaps are silent — code that runs without complaint but may diverge from Haskell. Phase B therefore becomes validation-driven rather than TODO-driven.

Conway governance ratification is the highest-value first target because:
- It is the most recently added subsystem (most likely to harbor subtle bugs)
- It runs every epoch on live Conway networks (regressions are user-visible)
- Koios exposes every input and expected output, making algorithm-in-isolation testing feasible without a full Mithril replay

## Non-goals

- Full Mithril-driven end-to-end replay (budget blowout; separate spec if needed)
- Non-ratified proposals beyond one negative case (expired / voted down)
- Enactment-side correctness beyond "right action in right `enacted_*` slot" (pparam updates actually taking effect on later blocks is out of scope)
- Threshold audits for code paths current preview proposals do not exercise
- Automated fixture refresh against live Koios (fixtures are captured once and committed)
- Byte-for-byte state equality with Haskell (fidelity bar C — equivalence at the observable level is sufficient)
- Reward calculations
- Chain reorgs mid-ratification
- **`Info`, `NoConfidence`, and `TreasuryWithdrawal` fixtures.** Positive fixtures for this first slice must target `PParamUpdate`, `HardFork`, `Committee`, or `Constitution` actions — the four action types that land in an `enacted_*` slot and can be asserted with a single-line `assert_eq!`. The other three action types have side-effect-only outcomes (treasury pot delta, `last_ratify_delayed` flag) that need distinct assertion paths; those are a follow-up sub-project.

## Success criteria

- At least one real preview proposal is replayed in a dugite test and produces the Haskell-identical enacted set
- At least one negative-case proposal (expired / voted down) is replayed and correctly not enacted
- Any divergence surfaced by the tests is either fixed in dugite or documented as a known limitation with rationale
- The test framework is reusable — adding a new fixture is a one-shot capture command plus a `#[test]` stub, no plumbing changes

## Architecture

Two components, cleanly separated:

### Component 1 — Fixture capture helper

A one-shot CLI tool that queries Koios (public preview endpoint, not the MCP integration) for everything needed to drive `ratify_proposals()` on a single proposal and writes a committed JSON fixture.

Location: `crates/dugite-cli/src/bin/capture_ratification_fixture.rs` (new binary) unless an `xtask` crate is already present (check during implementation).

Interface:
```
dugite-capture-ratification-fixture \
    --network preview \
    --proposal-id <hex_tx_hash>#<cert_index> \
    --output fixtures/conway-ratification/<id>.json
```

Implementation is a thin sequential wrapper around `reqwest` calls to `https://preview.koios.rest/api/v1`. No concurrency. Fails loud on any error, printing the failing URL and response.

### Component 2 — Test harness

An integration test in `crates/dugite-ledger/tests/conway_ratification.rs` that loads a fixture, constructs a minimal `LedgerState`, calls `ratify_proposals()`, and asserts the resulting enacted slot matches the fixture's `expected_outcome`. One `#[test]` per fixture. No Koios or network dependency at test time.

### Separation of concerns

- Fixture capture is offline dev tooling — it is **not** a test dependency and does not run in CI.
- The test harness is pure — reads JSON, runs logic, asserts.
- One fixture file = one test case. Adding proposals is additive.

## Data flow

### Capture flow

```
User picks proposal_id (preview, ratified or expired for negative case)
    ↓
capture_ratification_fixture runs:
    1. koios proposal_list             → proposal metadata + action
    2. koios proposal_voting_summary   → ratified/dropped, enacted epoch
    3. koios proposal_votes            → individual vote records
    4. koios drep_voting_power_history → DRep stake at (ratification_epoch - 1)
    5. koios pool_voting_power_history → SPO stake at (ratification_epoch - 1)
    6. koios committee_info            → members, threshold, min_size, expirations
    7. koios epoch_params              → full Conway pparams at ratification_epoch
    8. (if proposal has prev_action_id) recurse for parent to seed enacted_* anchor
    ↓
Serialize to pretty JSON → commit to repo
```

### Test flow

```
Test loads fixtures/conway-ratification/<id>.json
    ↓
Build minimal LedgerState:
    gov.governance.proposals           = { fixture.proposal }
    gov.governance.votes               = fixture.votes (grouped by voter_type)
    gov.governance.committee           = fixture.committee
    gov.governance.constitution        = None (or from fixture if relevant)
    gov.governance.cur_pparams         = fixture.pparams
    epochs.snapshots.set.pool_stake    = fixture.spo_stake
    gov.governance.drep_power_snapshot = fixture.drep_power
    delegations.vote_delegations       = {} (forces snapshot path in build_drep_power_cache)
    gov.governance.enacted_*           = fixture.parent_enacted (for prev_action_id chain)
    ↓
Call ledger.ratify_proposals()
    ↓
Assert:
    For positive fixtures (PParamUpdate / HardFork / Committee / Constitution only):
        gov.governance.enacted_<bucket> == Some(fixture.expected_outcome.enacted_id)
    For negative cases:
        no enacted_* slot contains the fixture's proposal ID
        AND the proposal still sits in gov.governance.proposals
        OR the proposal was dropped (matches fixture.expected_outcome.ratified == false)
```

## Fixture schema

```json
{
  "proposal": {
    "gov_action_id": "<tx_hash>#<cert_index>",
    "action": { "tag": "PParamUpdate", "fields": { ... } },
    "deposit": 100000000000,
    "return_addr": "stake1u...",
    "expiration": 142,
    "anchor": { "url": "...", "data_hash": "..." }
  },
  "votes": [
    { "voter_type": "ConstitutionalCommitteeHotKeyHash", "voter_id": "...", "vote": "Yes" },
    { "voter_type": "DRepKeyHash",                       "voter_id": "...", "vote": "No"  },
    { "voter_type": "StakePoolKeyHash",                  "voter_id": "...", "vote": "Abstain" }
  ],
  "drep_power": { "<drep_id_hex>": 12345678 },
  "spo_stake":  { "<pool_id_hex>": 87654321 },
  "committee": {
    "members":    [ { "cold_key": "...", "hot_key": "...", "expiration": 200 } ],
    "threshold":  { "numerator": 2, "denominator": 3 },
    "min_size":   0,
    "resigned":   []
  },
  "pparams": {
    "drep_voting_thresholds":   { "motion_no_confidence": 0.67, "...": "..." },
    "pool_voting_thresholds":   { "motion_no_confidence": 0.51, "...": "..." },
    "committee_min_size":       0,
    "committee_max_term":       365,
    "gov_action_lifetime":      6,
    "gov_action_deposit":       100000000000,
    "drep_deposit":             500000000,
    "drep_activity":            20,
    "...":                      "remaining Conway pparams"
  },
  "total_drep_stake": 123456789012,
  "total_spo_stake":  987654321098,
  "expected_outcome": {
    "ratified":       true,
    "enacted_bucket": "PParamUpdate",
    "enacted_epoch":  140,
    "enacted_id":     "<same as proposal.gov_action_id>"
  },
  "parent_enacted": {
    "PParamUpdate":  null,
    "HardFork":      null,
    "Committee":     null,
    "Constitution":  null
  }
}
```

## Components in detail

### `RatificationFixture` Rust type

Location: `crates/dugite-ledger/tests/common/ratification_fixture.rs` (new).

Shallow structs deriving `serde::Deserialize`, mirroring the JSON schema above. One `into_ledger_state(self) -> LedgerState` method that constructs the minimal state described in the test flow. DRep credential conversion reuses the `Credential::to_typed_hash32()` path seeded in Phase A Task 3.

### Assertion helper

```rust
fn assert_ratified(ledger: &LedgerState, expected: &ExpectedOutcome) {
    let gov = &ledger.gov.governance;
    // Only PParamUpdate / HardFork / Committee / Constitution are in scope; the
    // loader rejects fixtures with any other bucket so the match is exhaustive.
    let actual = match expected.enacted_bucket {
        Bucket::PParamUpdate => gov.enacted_pparam_update.as_ref(),
        Bucket::HardFork     => gov.enacted_hard_fork.as_ref(),
        Bucket::Committee    => gov.enacted_committee.as_ref(),
        Bucket::Constitution => gov.enacted_constitution.as_ref(),
    };
    assert_eq!(actual, Some(&expected.enacted_id),
        "bucket {:?}: expected {:?}, got {:?}",
        expected.enacted_bucket, expected.enacted_id, actual);
}

fn assert_not_ratified(ledger: &LedgerState, proposal_id: &GovActionId) {
    let gov = &ledger.gov.governance;
    for slot in [&gov.enacted_pparam_update, &gov.enacted_hard_fork,
                 &gov.enacted_committee, &gov.enacted_constitution] {
        assert_ne!(slot.as_ref(), Some(proposal_id));
    }
}
```

### Test harness pattern

```rust
#[test]
fn ratifies_preview_proposal_<sanitized_id>() {
    let fixture = RatificationFixture::load("fixtures/conway-ratification/<id>.json");
    let mut ledger = fixture.clone().into_ledger_state();
    ledger.ratify_proposals();
    assert_ratified(&ledger, &fixture.expected_outcome);
}

#[test]
fn rejects_preview_proposal_<negative_id>() {
    let fixture = RatificationFixture::load("fixtures/conway-ratification/<negative_id>.json");
    let proposal_id = fixture.proposal.gov_action_id.clone();
    let mut ledger = fixture.into_ledger_state();
    ledger.ratify_proposals();
    assert_not_ratified(&ledger, &proposal_id);
}
```

One `#[test]` per fixture, generated manually. When fixture count exceeds ~5, revisit a `test_each`-style macro.

## Edge cases

- **Parent chains (`prev_action_id`).** Proposals reference a parent enacted ID. The capture helper recurses once to seed `parent_enacted.<bucket>`; the test loader writes those into `enacted_*` before calling `ratify_proposals()`. Without this, prev_action_id validation rejects the proposal.
- **Committee state at ratification epoch.** The captured committee must match Haskell's view at epoch-boundary time, not live state at capture time. `koios_committee_info` with an explicit epoch parameter.
- **Late-submitted proposals.** DRep/SPO power snapshots are taken at `ratification_epoch - 1` (matching dugite's `set` snapshot convention at `governance.rs:658-666`). The fixture documents which epoch the snapshot was taken at.
- **Bech32 vs hex.** Koios returns DRep and pool IDs as bech32; fixtures store hex; the loader converts. Single canonical form (hex) simplifies fixture diffing.
- **`ratification_snapshot` left `None`.** The fixture drives live state, not the ratification snapshot. This keeps the test focused on the ratify algorithm's live-state path.
- **Zero-vote proposals.** Some proposals reach ratification window with zero votes (auto-declined or auto-accepted depending on action type). Fixture carries an empty `votes: []`; the test verifies correct handling.

## Divergence classification

When a test fails, the failure falls into one of three buckets:

1. **Wrong outcome** — dugite ratifies when Haskell didn't (or vice versa). Most likely a threshold calculation or quorum bug. Investigation starts at `governance.rs::ratify_proposals` thresholding.
2. **Right outcome, wrong bucket** — the proposal is enacted into the wrong `enacted_*` slot. Likely a `GovAction` → bucket mapping bug in `enact_gov_action`.
3. **Fixture defect** — captured data is stale or wrong (e.g., epoch off-by-one on DRep power). Fix the capture helper, re-run, re-commit.

Triage: run with `RUST_LOG=dugite_ledger::state::governance=trace` to see per-proposal threshold math, compare line-by-line against Haskell `cardano-ledger/Conway/Rules/Ratify.hs`.

## Testing strategy

1. **Fixture smoke test** — load a known fixture, construct `LedgerState`, assert `gov.governance.proposals.len() == 1`. Validates plumbing before asserting algorithm correctness.
2. **Happy-path ratification** — 1-3 fixtures for real preview proposals that reached the `enacted` state. Each is one `#[test]`.
3. **Negative case** — at least one fixture for a proposal that did NOT ratify (expired with insufficient votes or voted down). Catches false-positive enactment bugs.
4. **No unit tests for `RatificationFixture` itself** — correctness is implicitly verified by the integration tests that consume it.

## Risk / tradeoffs

- **Synthetic state.** The tests drive algorithm-in-isolation, not full end-to-end replay. A bug that only manifests when interacting with other state (e.g., `process_epoch_transition` ordering with rewards or snapshot rotation) would not be caught here. This is accepted for the narrow scope; broader replay is a separate spec.
- **Fixture capture dependency on Koios availability.** If Koios changes its JSON schema, captured fixtures may become uncapturable (but remain loadable — they are committed). Acceptable for one-shot tooling.
- **Fixtures go stale.** Preview proposals can disappear from Koios history after long enough; fixtures are static and committed so this doesn't affect the tests, only the ability to re-capture. Acceptable.
- **Preview-only coverage.** Mainnet Conway proposals are similar but not identical in scale. The test framework is trivially reusable for mainnet fixtures once we have a reason to capture them.
- **Threshold calculation is the most likely divergence point.** Rational arithmetic in dugite may differ from Haskell's `UnitInterval` arithmetic by rounding. If tests fail on rounding edges, document as bar-C tolerance or tighten `UnitInterval` representation — decision deferred to discovery time.

## Done when

- `capture_ratification_fixture` binary builds and successfully captures at least 2 real preview proposal fixtures (1 positive, 1 negative)
- `crates/dugite-ledger/tests/conway_ratification.rs` contains at least 2 `#[test]` functions and they pass against dugite's current `ratify_proposals()` (or, if they fail, the fix or documentation of the limitation is landed in the same PR)
- Clippy + fmt + nextest clean
- Fixtures committed under `fixtures/conway-ratification/`
- A brief follow-up note documenting the capture command, so adding the next fixture is one-shot

## Follow-ups (out of scope for this spec)

- Broaden to mainnet fixtures
- Add threshold audit suite (medium-scope Phase B sub-project)
- Add full Mithril-replay end-to-end ratification test (broad-scope Phase B sub-project)
- Reward calculation cross-validation (separate spec)
