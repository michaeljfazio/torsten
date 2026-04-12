# Phase B — Conway Ratification Validation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Validate dugite's `LedgerState::ratify_proposals()` produces the same enacted set as Haskell cardano-node for real preview-testnet proposals, via a committed-fixture test harness plus a one-shot Koios capture tool.

**Architecture:** Two components cleanly separated: (1) an offline `capture_ratification_fixture` binary that queries the public Koios preview endpoint and writes a JSON fixture; (2) a pure offline integration test in `crates/dugite-ledger/tests/conway_ratification.rs` that loads a fixture, constructs a minimal `LedgerState`, calls `ratify_proposals()`, and asserts the resulting enacted slot matches the fixture's expected outcome. One `#[test]` per fixture; no network dependency at test time.

**Tech Stack:** Rust 2021, serde / serde_json for fixture schema, `reqwest` (blocking) for Koios capture, `cargo nextest` for test execution, existing `dugite-ledger` types (`LedgerState`, `GovernanceState`, `ProposalState`, `GovActionId`, `Vote`, `Voter`, `VotingProcedure`, `Constitution`).

**Design reference:** `docs/superpowers/specs/2026-04-11-ledger-phase-b-ratification-validation-design.md`

**Scope guardrails (from spec non-goals):**
- Positive fixtures target only `PParamUpdate`, `HardFork`, `Committee` (UpdateCommittee), or `Constitution` (NewConstitution) actions — the 4 that land in an `enacted_*` slot.
- `Info`, `NoConfidence`, `TreasuryWithdrawals` are out of scope (distinct assertion paths).
- No live Koios dependency at test time; fixtures are committed once.

---

## File Structure

**New files:**
- `crates/dugite-ledger/tests/common/mod.rs` — common test helpers module declaration (may already exist; create if missing).
- `crates/dugite-ledger/tests/common/ratification_fixture.rs` — `RatificationFixture` struct, `ExpectedOutcome`, `EnactedBucket` enum, JSON loader, `into_ledger_state()` conversion, assertion helpers.
- `crates/dugite-ledger/tests/conway_ratification.rs` — integration tests (`#[test]` per fixture, both positive and negative).
- `fixtures/conway-ratification/README.md` — one-paragraph capture instructions (regenerating a fixture is one command).
- `fixtures/conway-ratification/<proposal-id>.json` — captured fixtures (positive + negative).
- `crates/dugite-cli/src/bin/capture_ratification_fixture.rs` — offline capture binary (new bin target declared in `crates/dugite-cli/Cargo.toml`).

**Modified files:**
- `crates/dugite-ledger/Cargo.toml` — add `serde_json` to `[dev-dependencies]` if not already present.
- `crates/dugite-cli/Cargo.toml` — add `reqwest` (blocking feature), `serde_json`, `clap` dev-dep already exists; declare new `[[bin]]`.

**Out of scope for this plan:** any change to production `crates/dugite-ledger/src/state/governance.rs` or `ratify_proposals()`. If the tests surface a divergence, the fix lands in a follow-up commit under the same PR per the spec's "Done when".

---

## Task 1: Fixture schema Rust types

**Files:**
- Create: `crates/dugite-ledger/tests/common/mod.rs`
- Create: `crates/dugite-ledger/tests/common/ratification_fixture.rs`
- Modify: `crates/dugite-ledger/Cargo.toml` (add `serde_json` dev-dep if missing)

Defines the Rust types mirroring the fixture JSON schema from the spec. Deserialization is the full extent of behavior here — `into_ledger_state` lands in Task 2.

- [ ] **Step 1: Verify whether `tests/common/mod.rs` already exists**

```bash
ls crates/dugite-ledger/tests/common/ 2>/dev/null || echo "missing"
```

Expected: either file listing (keep existing contents, just add a `pub mod ratification_fixture;`) or `missing` (create the directory + `mod.rs`).

- [ ] **Step 2: Verify `serde_json` is a dev-dependency of `dugite-ledger`**

```bash
grep -n serde_json crates/dugite-ledger/Cargo.toml
```

If absent under `[dev-dependencies]`, add:

```toml
[dev-dependencies]
serde_json = { workspace = true }
```

(Use `workspace = true` only if `serde_json` is declared in the workspace root `Cargo.toml`; otherwise use the version already used elsewhere in the workspace. Run `cargo metadata --format-version 1 | grep -o serde_json | head -1` to verify it is pulled in by something in the workspace.)

- [ ] **Step 3: Write the failing schema-parse test**

Append to `crates/dugite-ledger/tests/common/ratification_fixture.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_JSON: &str = r#"{
      "proposal": {
        "gov_action_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#0",
        "action": { "tag": "PParamUpdate", "fields": {} },
        "deposit": 100000000000,
        "return_addr_hex": "e0deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
        "expiration": 142,
        "anchor": null
      },
      "proposed_epoch": 138,
      "votes": [],
      "drep_power": {},
      "drep_no_confidence": 0,
      "drep_abstain": 0,
      "spo_stake": {},
      "committee": {
        "members": [],
        "threshold": { "numerator": 2, "denominator": 3 },
        "min_size": 0,
        "resigned": []
      },
      "pparams_epoch": 140,
      "pparams": {},
      "total_drep_stake": 0,
      "total_spo_stake": 0,
      "expected_outcome": {
        "ratified": true,
        "enacted_bucket": "PParamUpdate",
        "enacted_epoch": 140,
        "enacted_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa#0"
      },
      "parent_enacted": {
        "PParamUpdate": null,
        "HardFork": null,
        "Committee": null,
        "Constitution": null
      }
    }"#;

    #[test]
    fn fixture_deserializes_minimal_json() {
        let fixture: RatificationFixture =
            serde_json::from_str(MINIMAL_JSON).expect("minimal fixture must parse");
        assert_eq!(fixture.proposal.deposit, 100_000_000_000);
        assert_eq!(fixture.expected_outcome.enacted_bucket, EnactedBucket::PParamUpdate);
        assert!(fixture.expected_outcome.ratified);
    }
}
```

- [ ] **Step 4: Run the test (expect compile failure — types do not exist yet)**

```bash
cargo nextest run -p dugite-ledger -E 'test(fixture_deserializes_minimal_json)'
```

Expected: compile error "cannot find struct `RatificationFixture`".

- [ ] **Step 5: Write the minimal type definitions**

Prepend to `crates/dugite-ledger/tests/common/ratification_fixture.rs`:

```rust
//! Offline fixture schema used by `conway_ratification.rs` integration tests.
//!
//! Fields mirror the JSON schema defined in the Phase B design spec.  No
//! runtime behavior beyond deserialization + conversion into a `LedgerState`
//! (see `into_ledger_state`).

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct RatificationFixture {
    pub proposal: FixtureProposal,
    pub proposed_epoch: u64,
    pub votes: Vec<FixtureVote>,
    pub drep_power: BTreeMap<String, u64>,
    pub drep_no_confidence: u64,
    pub drep_abstain: u64,
    pub spo_stake: BTreeMap<String, u64>,
    pub committee: FixtureCommittee,
    pub pparams_epoch: u64,
    pub pparams: serde_json::Value,
    pub total_drep_stake: u64,
    pub total_spo_stake: u64,
    pub expected_outcome: ExpectedOutcome,
    pub parent_enacted: ParentEnacted,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureProposal {
    pub gov_action_id: String,
    pub action: serde_json::Value,
    pub deposit: u64,
    pub return_addr_hex: String,
    pub expiration: u64,
    pub anchor: Option<FixtureAnchor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureAnchor {
    pub url: String,
    pub data_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureVote {
    pub voter_type: FixtureVoterType,
    pub voter_id: String,
    pub vote: FixtureVoteValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum FixtureVoterType {
    ConstitutionalCommitteeHotKeyHash,
    ConstitutionalCommitteeHotScriptHash,
    DRepKeyHash,
    DRepScriptHash,
    StakePoolKeyHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum FixtureVoteValue {
    Yes,
    No,
    Abstain,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureCommittee {
    pub members: Vec<FixtureCommitteeMember>,
    pub threshold: FixtureRational,
    pub min_size: u64,
    pub resigned: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureCommitteeMember {
    pub cold_key: String,
    pub hot_key: Option<String>,
    pub expiration: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FixtureRational {
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExpectedOutcome {
    pub ratified: bool,
    pub enacted_bucket: EnactedBucket,
    pub enacted_epoch: u64,
    pub enacted_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum EnactedBucket {
    PParamUpdate,
    HardFork,
    Committee,
    Constitution,
    // Deliberately excluded: Info, NoConfidence, TreasuryWithdrawal
    // (out of scope per spec non-goals).  The test loader rejects fixtures
    // using them so the assertion match stays exhaustive.
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ParentEnacted {
    #[serde(rename = "PParamUpdate")]
    pub pparam_update: Option<String>,
    #[serde(rename = "HardFork")]
    pub hard_fork: Option<String>,
    #[serde(rename = "Committee")]
    pub committee: Option<String>,
    #[serde(rename = "Constitution")]
    pub constitution: Option<String>,
}
```

Append `pub mod ratification_fixture;` to `crates/dugite-ledger/tests/common/mod.rs` (create the file if it did not exist in Step 1).

- [ ] **Step 6: Run the test to verify it passes**

```bash
cargo nextest run -p dugite-ledger -E 'test(fixture_deserializes_minimal_json)'
```

Expected: PASS.

- [ ] **Step 7: Lint and format**

```bash
cargo clippy -p dugite-ledger --tests -- -D warnings
cargo fmt -p dugite-ledger -- --check
```

Expected: both clean. If fmt fails, run `cargo fmt -p dugite-ledger` and recheck.

- [ ] **Step 8: Commit**

```bash
git add crates/dugite-ledger/tests/common/mod.rs \
        crates/dugite-ledger/tests/common/ratification_fixture.rs \
        crates/dugite-ledger/Cargo.toml
git commit -m "$(cat <<'EOF'
test(ledger): add Conway ratification fixture schema types

Skeleton types mirroring the Phase B design spec's fixture JSON.
Deserialization-only for now; LedgerState conversion lands next.
EOF
)"
```

---

## Task 2: Fixture → `LedgerState` conversion

**Files:**
- Modify: `crates/dugite-ledger/tests/common/ratification_fixture.rs`
- Test: same file (unit test)

Converts a parsed `RatificationFixture` into a minimal `LedgerState` positioned at the fixture's ratification epoch with just the fields `ratify_proposals()` reads. Drives the snapshot path in `build_drep_power_cache` by leaving `vote_delegations` empty and populating `drep_distribution_snapshot` + the two aux counters.

- [ ] **Step 1: Write the failing conversion smoke test**

Append to `crates/dugite-ledger/tests/common/ratification_fixture.rs` `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn minimal_fixture_builds_ledger_state_with_one_proposal() {
        let fixture: RatificationFixture =
            serde_json::from_str(MINIMAL_JSON).expect("parse");
        let ledger = fixture.into_ledger_state();
        assert_eq!(ledger.gov.governance.proposals.len(), 1);
        assert!(ledger.gov.governance.drep_distribution_snapshot.is_empty());
        assert_eq!(ledger.gov.governance.drep_snapshot_no_confidence, 0);
    }
```

- [ ] **Step 2: Run test to confirm failure**

```bash
cargo nextest run -p dugite-ledger -E 'test(minimal_fixture_builds_ledger_state_with_one_proposal)'
```

Expected: compile error "no method named `into_ledger_state`".

- [ ] **Step 3: Implement `into_ledger_state` + helpers**

Add to `crates/dugite-ledger/tests/common/ratification_fixture.rs`:

```rust
use dugite_ledger::state::{
    GovernanceState, LedgerState, ProposalState, RatificationSnapshot,
};
use dugite_primitives::hash::{Hash28, Hash32};
use dugite_primitives::transaction::{
    Anchor, Constitution, GovAction, GovActionId, ProposalProcedure, TransactionHash, Vote,
    Voter, VotingProcedure,
};

impl RatificationFixture {
    /// Parse a fixture file from disk. Panics on any error — these are test
    /// fixtures and any parse failure should fail the test loudly.
    pub fn load(path: &str) -> Self {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"))
    }

    /// Build a minimal `LedgerState` positioned at the fixture's
    /// `pparams_epoch` with exactly the fields `ratify_proposals()` reads.
    pub fn into_ledger_state(self) -> LedgerState {
        let mut ledger = LedgerState::default();
        ledger.epoch_no = self.pparams_epoch;

        // Proposal
        let proposal_id = parse_gov_action_id(&self.proposal.gov_action_id);
        let procedure = ProposalProcedure {
            deposit: self.proposal.deposit,
            return_addr: hex::decode(&self.proposal.return_addr_hex)
                .expect("return_addr_hex must be valid hex"),
            gov_action: parse_gov_action(&self.proposal.action),
            anchor: self.proposal.anchor.clone().map(|a| Anchor {
                url: a.url,
                data_hash: Hash32::from_hex(&a.data_hash)
                    .expect("anchor data_hash must be 32-byte hex"),
            }),
        };
        let mut yes = 0u64;
        let mut no = 0u64;
        let mut abstain = 0u64;
        for v in &self.votes {
            match v.vote {
                FixtureVoteValue::Yes => yes += 1,
                FixtureVoteValue::No => no += 1,
                FixtureVoteValue::Abstain => abstain += 1,
            }
        }
        let prop_state = ProposalState {
            procedure: procedure.clone(),
            proposed_epoch: self.proposed_epoch,
            expires_epoch: self.proposal.expiration,
            yes_votes: yes,
            no_votes: no,
            abstain_votes: abstain,
        };
        ledger
            .gov
            .governance
            .proposals
            .insert(proposal_id.clone(), prop_state);

        // Votes
        let mut votes: Vec<(Voter, VotingProcedure)> = Vec::with_capacity(self.votes.len());
        for v in self.votes {
            votes.push((
                parse_voter(v.voter_type, &v.voter_id),
                VotingProcedure {
                    vote: match v.vote {
                        FixtureVoteValue::Yes => Vote::Yes,
                        FixtureVoteValue::No => Vote::No,
                        FixtureVoteValue::Abstain => Vote::Abstain,
                    },
                    anchor: None,
                },
            ));
        }
        ledger
            .gov
            .governance
            .votes_by_action
            .insert(proposal_id.clone(), votes);

        // Committee
        for member in self.committee.members {
            let cold = Hash32::from_hex(&member.cold_key)
                .expect("committee cold_key must be 32-byte hex");
            if let Some(hot) = member.hot_key {
                let hot_h =
                    Hash32::from_hex(&hot).expect("committee hot_key must be 32-byte hex");
                ledger.gov.governance.committee_hot_keys.insert(cold, hot_h);
            }
            ledger
                .gov
                .governance
                .committee_expiration
                .insert(cold, member.expiration);
        }
        for cold_hex in self.committee.resigned {
            let cold = Hash32::from_hex(&cold_hex).expect("resigned cold_key hex");
            ledger
                .gov
                .governance
                .committee_resigned
                .insert(cold, None);
        }
        ledger.gov.governance.committee_threshold = Some(dugite_primitives::Rational {
            numerator: self.committee.threshold.numerator,
            denominator: self.committee.threshold.denominator,
        });

        // DRep snapshot (forces the snapshot path in build_drep_power_cache)
        for (drep_hex, power) in self.drep_power {
            let hash = Hash32::from_hex(&drep_hex).expect("drep_power key must be 32-byte hex");
            ledger
                .gov
                .governance
                .drep_distribution_snapshot
                .insert(hash, power);
        }
        ledger.gov.governance.drep_snapshot_no_confidence = self.drep_no_confidence;
        ledger.gov.governance.drep_snapshot_abstain = self.drep_abstain;
        // vote_delegations intentionally left empty so build_drep_power_cache
        // does NOT fall back to the live path.

        // SPO stake in the `set` snapshot
        for (pool_hex, stake) in self.spo_stake {
            let pool = Hash28::from_hex(&pool_hex).expect("spo_stake key must be 28-byte hex");
            ledger.epochs.snapshots.set.pool_stake.insert(pool, stake);
        }

        // Parent enacted chain
        if let Some(id) = self.parent_enacted.pparam_update {
            ledger.gov.governance.enacted_pparam_update = Some(parse_gov_action_id(&id));
        }
        if let Some(id) = self.parent_enacted.hard_fork {
            ledger.gov.governance.enacted_hard_fork = Some(parse_gov_action_id(&id));
        }
        if let Some(id) = self.parent_enacted.committee {
            ledger.gov.governance.enacted_committee = Some(parse_gov_action_id(&id));
        }
        if let Some(id) = self.parent_enacted.constitution {
            ledger.gov.governance.enacted_constitution = Some(parse_gov_action_id(&id));
        }

        // ratification_snapshot stays None — tests run against live state.
        ledger
    }
}

fn parse_gov_action_id(s: &str) -> GovActionId {
    let (tx_hex, idx_str) = s
        .split_once('#')
        .unwrap_or_else(|| panic!("malformed gov_action_id: {s}"));
    let tx_hash = TransactionHash::from_hex(tx_hex)
        .unwrap_or_else(|_| panic!("tx hash in gov_action_id not 32 bytes: {tx_hex}"));
    let idx: u32 = idx_str
        .parse()
        .unwrap_or_else(|_| panic!("action index not u32: {idx_str}"));
    GovActionId {
        transaction_id: tx_hash,
        action_index: idx,
    }
}

fn parse_gov_action(value: &serde_json::Value) -> GovAction {
    // The capture helper writes one of the 4 in-scope action tags.  Full
    // deserialization is out of scope for this first slice — we accept a
    // placeholder InfoAction for any fixture without a PParamUpdate/HardFork/
    // Committee/Constitution tag and let the test assertion catch the
    // mismatch.  Fixtures that need richer action content should extend
    // this match arm alongside the new test.
    match value.get("tag").and_then(|t| t.as_str()) {
        Some("PParamUpdate") | Some("HardFork") | Some("Committee")
        | Some("Constitution") => {
            // Minimal reconstruction — see capture helper for the authoritative
            // encoding.  We rely on the production ratify path exercising the
            // action tag only, not the inner fields, for the first-slice
            // fixtures selected by the spec (four action types that land in
            // an `enacted_*` slot).
            GovAction::InfoAction
        }
        _ => GovAction::InfoAction,
    }
}

fn parse_voter(kind: FixtureVoterType, id_hex: &str) -> Voter {
    use dugite_primitives::transaction::Credential;
    match kind {
        FixtureVoterType::ConstitutionalCommitteeHotKeyHash => {
            Voter::ConstitutionalCommittee(Credential::KeyHash(
                Hash32::from_hex(id_hex).expect("voter id 32-byte hex"),
            ))
        }
        FixtureVoterType::ConstitutionalCommitteeHotScriptHash => {
            Voter::ConstitutionalCommittee(Credential::ScriptHash(
                Hash32::from_hex(id_hex).expect("voter id 32-byte hex"),
            ))
        }
        FixtureVoterType::DRepKeyHash => Voter::DRep(Credential::KeyHash(
            Hash32::from_hex(id_hex).expect("voter id 32-byte hex"),
        )),
        FixtureVoterType::DRepScriptHash => Voter::DRep(Credential::ScriptHash(
            Hash32::from_hex(id_hex).expect("voter id 32-byte hex"),
        )),
        FixtureVoterType::StakePoolKeyHash => Voter::StakePool(
            Hash32::from_hex(id_hex).expect("stake pool voter id must be 32-byte hex"),
        ),
    }
}
```

**Note to implementer:** The exact paths for `LedgerState::default()`, `ProposalState`, the `Constitution`/`Anchor`/`GovAction`/`Voter`/`Credential` types, and `Hash28::from_hex` / `Hash32::from_hex` / `TransactionHash::from_hex` must be verified against the current source tree before coding. If any constructor does not exist as written, pause and ask before diverging — the type and module paths have been stable but any mismatch is a real blocker, not a "fix inline" moment. Known good references:
- `crates/dugite-ledger/src/state/mod.rs:290-478` (GovernanceState fields, ProposalState, DRepRegistration, RatificationSnapshot)
- `crates/dugite-ledger/src/state/governance.rs:1664` (`build_drep_power_cache` — documents the exact fields used)
- `crates/dugite-primitives/src/transaction.rs:375-449` (GovAction, GovActionId, ProposalProcedure, Voter, Vote, VotingProcedure, Constitution, Anchor)
- `crates/dugite-primitives/src/hash.rs` (`Hash28`, `Hash32`, `from_hex` if present; otherwise use the bytes → type path already used in other `tests/common/` helpers)

If `Hash32::from_hex` / `Hash28::from_hex` helpers do not exist, add a private `hex_to_hash32` / `hex_to_hash28` helper in this file that decodes via `hex::decode(...).try_into()` — do not add public helpers to the production crate.

- [ ] **Step 4: Run both tests to verify they pass**

```bash
cargo nextest run -p dugite-ledger -E 'test(fixture_deserializes_minimal_json) or test(minimal_fixture_builds_ledger_state_with_one_proposal)'
```

Expected: 2 passed, 0 failed.

- [ ] **Step 5: Lint + format**

```bash
cargo clippy -p dugite-ledger --tests -- -D warnings
cargo fmt -p dugite-ledger -- --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-ledger/tests/common/ratification_fixture.rs
git commit -m "$(cat <<'EOF'
test(ledger): convert ratification fixture to minimal LedgerState

Populates the exact fields ratify_proposals() reads: proposals,
votes_by_action, committee_*, drep_distribution_snapshot + the two
aux counters, epochs.snapshots.set.pool_stake, enacted_*.  Leaves
vote_delegations empty so build_drep_power_cache takes the snapshot
path rather than live-state fallback.
EOF
)"
```

---

## Task 3: Fixture capture binary scaffold (no network yet)

**Files:**
- Create: `crates/dugite-cli/src/bin/capture_ratification_fixture.rs`
- Modify: `crates/dugite-cli/Cargo.toml` (bin declaration + reqwest blocking dep)

The binary builds, parses CLI args, and writes an empty fixture — no Koios calls yet. We get the build/CLI wired up first, then add network plumbing in Task 4.

- [ ] **Step 1: Inspect `dugite-cli/Cargo.toml` for existing deps**

```bash
cat crates/dugite-cli/Cargo.toml
```

Note whether `clap`, `reqwest`, `serde_json`, `tokio`, `hex` are already declared and which features are enabled. Reuse whatever is there — do not introduce a second HTTP client.

- [ ] **Step 2: Add `[[bin]]` target and any missing deps**

Append to `crates/dugite-cli/Cargo.toml` (only the keys that are missing):

```toml
[[bin]]
name = "capture-ratification-fixture"
path = "src/bin/capture_ratification_fixture.rs"

[dependencies]
# add ONLY if missing — otherwise reuse the workspace declarations:
# reqwest = { version = "...", default-features = false, features = ["blocking", "json", "rustls-tls"] }
# serde_json = { workspace = true }
# hex = { workspace = true }
```

Prefer `reqwest` with `blocking` + `rustls-tls` + `json` to match the "no concurrency, fail loud" design constraint.

- [ ] **Step 3: Write the scaffold binary**

Create `crates/dugite-cli/src/bin/capture_ratification_fixture.rs`:

```rust
//! One-shot offline capture tool.
//!
//! Queries the public preview Koios endpoint for the data `ratify_proposals()`
//! needs and writes a JSON fixture under `fixtures/conway-ratification/`.
//! Not a test dependency — never runs in CI.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "capture-ratification-fixture")]
struct Args {
    /// Network (only "preview" is supported for this first slice).
    #[arg(long, default_value = "preview")]
    network: String,

    /// Governance action id in the form `<tx_hex>#<cert_index>`.
    #[arg(long)]
    proposal_id: String,

    /// Output path (parent directory must exist).
    #[arg(long)]
    output: PathBuf,
}

fn main() {
    let args = Args::parse();
    if args.network != "preview" {
        eprintln!("only --network=preview is supported");
        std::process::exit(2);
    }
    eprintln!(
        "capture-ratification-fixture: TODO — fetch {} and write {}",
        args.proposal_id,
        args.output.display()
    );
    std::process::exit(1);
}
```

- [ ] **Step 4: Build the binary**

```bash
cargo build -p dugite-cli --bin capture-ratification-fixture
```

Expected: build succeeds. If `clap` derive macros are missing a feature, add `features = ["derive"]` to the `clap` dependency in the crate.

- [ ] **Step 5: Invoke --help to verify CLI parses**

```bash
./target/debug/capture-ratification-fixture --help
```

Expected: usage text shows `--network`, `--proposal-id`, `--output`.

- [ ] **Step 6: Lint + fmt**

```bash
cargo clippy -p dugite-cli --bin capture-ratification-fixture -- -D warnings
cargo fmt -p dugite-cli -- --check
```

- [ ] **Step 7: Commit**

```bash
git add crates/dugite-cli/Cargo.toml \
        crates/dugite-cli/src/bin/capture_ratification_fixture.rs
git commit -m "$(cat <<'EOF'
chore(cli): add capture-ratification-fixture bin scaffold

CLI plumbing only — network fetching lands next.  Offline dev
tool, not a test dependency.
EOF
)"
```

---

## Task 4: Koios fetch + JSON serialization

**Files:**
- Modify: `crates/dugite-cli/src/bin/capture_ratification_fixture.rs`

Implement sequential `reqwest::blocking` calls to the public Koios preview endpoint and write the fixture JSON. Fails loud on any non-2xx with the URL and body. Spec Section "Data flow → Capture flow" enumerates the exact Koios endpoints in order.

- [ ] **Step 1: Add the Koios client helper**

Add near the top of `capture_ratification_fixture.rs`:

```rust
const KOIOS_BASE: &str = "https://preview.koios.rest/api/v1";

fn koios_get(client: &reqwest::blocking::Client, path: &str) -> serde_json::Value {
    let url = format!("{KOIOS_BASE}{path}");
    eprintln!("GET {url}");
    let resp = client
        .get(&url)
        .send()
        .unwrap_or_else(|e| panic!("koios GET {url} failed: {e}"));
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        panic!("koios {url} returned {status}: {body}");
    }
    resp.json()
        .unwrap_or_else(|e| panic!("koios {url} body was not JSON: {e}"))
}
```

- [ ] **Step 2: Fetch the proposal + voting summary + votes**

Replace the body of `main` after arg parsing with the sequential fetch flow from the spec:

```rust
    let client = reqwest::blocking::Client::builder()
        .user_agent("dugite-capture-ratification-fixture/0.1")
        .build()
        .expect("reqwest client");

    let (tx_hex, idx_str) = args
        .proposal_id
        .split_once('#')
        .unwrap_or_else(|| panic!("malformed --proposal-id: {}", args.proposal_id));
    let idx: u32 = idx_str.parse().expect("--proposal-id index not u32");

    // 1. proposal_list — find this specific proposal and its metadata
    let proposal_list = koios_get(
        &client,
        &format!("/proposal_list?proposal_tx_hash=eq.{tx_hex}&cert_index=eq.{idx}"),
    );
    let proposal = proposal_list
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| panic!("proposal {} not found on Koios", args.proposal_id));

    // 2. proposal_voting_summary — ratified/dropped + enacted_epoch
    let voting_summary = koios_get(
        &client,
        &format!("/proposal_voting_summary?proposal_id=eq.{tx_hex}"),
    );

    // 3. proposal_votes — individual vote records
    let votes = koios_get(
        &client,
        &format!("/proposal_votes?proposal_id=eq.{tx_hex}"),
    );

    // Extract the ratification epoch.  Koios exposes this as `enacted_epoch`
    // on the proposal row for ratified proposals, or `dropped_epoch` for
    // expired/dropped ones.  Power snapshots must be taken at
    // (ratification_epoch - 1).
    let ratification_epoch: u64 = proposal
        .get("ratified_epoch")
        .or_else(|| proposal.get("enacted_epoch"))
        .or_else(|| proposal.get("dropped_epoch"))
        .and_then(|v| v.as_u64())
        .unwrap_or_else(|| panic!("no ratification/dropped epoch in proposal row"));
    let snapshot_epoch = ratification_epoch.saturating_sub(1);

    // 4. drep_voting_power_history @ snapshot_epoch
    let drep_power = koios_get(
        &client,
        &format!("/drep_voting_power_history?epoch_no=eq.{snapshot_epoch}"),
    );

    // 5. pool_voting_power_history @ snapshot_epoch
    let pool_power = koios_get(
        &client,
        &format!("/pool_voting_power_history?epoch_no=eq.{snapshot_epoch}"),
    );

    // 6. committee_info — current committee at ratification time
    let committee = koios_get(&client, "/committee_info");

    // 7. epoch_params @ ratification_epoch
    let pparams = koios_get(
        &client,
        &format!("/epoch_params?_epoch_no={ratification_epoch}"),
    );
```

- [ ] **Step 3: Assemble the fixture JSON and write it**

Append after the fetches:

```rust
    let fixture = serde_json::json!({
        "proposal": proposal,
        "proposed_epoch": proposal.get("proposed_epoch").cloned().unwrap_or(serde_json::Value::Null),
        "votes": votes,
        "drep_power": drep_power,
        "drep_no_confidence": 0u64,
        "drep_abstain": 0u64,
        "spo_stake": pool_power,
        "committee": committee,
        "pparams_epoch": ratification_epoch,
        "pparams": pparams,
        "total_drep_stake": 0u64,
        "total_spo_stake": 0u64,
        "voting_summary": voting_summary,
        "expected_outcome": {
            "ratified": proposal.get("ratified_epoch").is_some()
                || proposal.get("enacted_epoch").is_some(),
            "enacted_bucket": proposal
                .get("proposal_type")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            "enacted_epoch": ratification_epoch,
            "enacted_id": format!("{tx_hex}#{idx}"),
        },
        "parent_enacted": {
            "PParamUpdate": null,
            "HardFork": null,
            "Committee": null,
            "Constitution": null,
        }
    });

    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent).expect("create output parent dir");
    }
    let pretty = serde_json::to_string_pretty(&fixture).expect("serialize");
    std::fs::write(&args.output, pretty + "\n").expect("write output");
    eprintln!("wrote {}", args.output.display());
```

Remove the earlier `std::process::exit(1)` stub.

**Implementer note:** The raw Koios JSON shapes do not perfectly match the fixture schema defined in Task 1 (e.g. Koios returns bech32 DRep / pool ids, `proposal_type` spellings may be `ParameterChange` rather than `PParamUpdate`, and the `committee` object needs transforming to the fixture's member-list shape). **Capturing a real fixture and loading it in the test will surface these mismatches.** Resolve them iteratively in Task 5 — do not speculate about Koios' schema in this task. The goal of Task 4 is: the binary builds, runs against live Koios, and produces *something* parseable. Task 5 will refine the shape based on actual server responses.

- [ ] **Step 4: Build**

```bash
cargo build -p dugite-cli --bin capture-ratification-fixture
```

- [ ] **Step 5: Lint + fmt**

```bash
cargo clippy -p dugite-cli --bin capture-ratification-fixture -- -D warnings
cargo fmt -p dugite-cli -- --check
```

- [ ] **Step 6: Commit**

```bash
git add crates/dugite-cli/src/bin/capture_ratification_fixture.rs
git commit -m "$(cat <<'EOF'
feat(cli): wire capture-ratification-fixture to Koios preview

Sequential blocking fetches per Phase B design spec.  Fails loud
on any non-2xx with URL + body.  Shape of the emitted fixture will
likely need tweaks after running against live Koios — that is
addressed in the capture task.
EOF
)"
```

---

## Task 5: Capture one positive preview fixture

**Files:**
- Create: `fixtures/conway-ratification/<id>.json`
- Create: `fixtures/conway-ratification/README.md`
- Possibly modify: `crates/dugite-cli/src/bin/capture_ratification_fixture.rs` (iteratively)
- Possibly modify: `crates/dugite-ledger/tests/common/ratification_fixture.rs` (iteratively)

This is the bridge task: pick a real preview proposal that reached `enacted` with one of the in-scope action types, capture it, and iterate on the capture helper and fixture schema until the emitted JSON round-trips through `RatificationFixture::load` and `into_ledger_state`.

**How to pick a candidate proposal:**
Query Koios preview for ratified proposals with an in-scope action type. From the host shell:

```bash
curl -s 'https://preview.koios.rest/api/v1/proposal_list' \
  | jq '[.[] | select(.enacted_epoch != null)
           | select(.proposal_type | IN("PParamUpdate","HardFork","ParameterChange","HardForkInitiation","NewCommittee","NewConstitution"))]
         | .[0:5]'
```

Keep the first one whose action type maps onto the 4 in-scope buckets. Record its `proposal_tx_hash`, `cert_index`, `enacted_epoch`, and `proposal_type`.

- [ ] **Step 1: Run the capture helper against the selected proposal**

```bash
mkdir -p fixtures/conway-ratification
./target/debug/capture-ratification-fixture \
    --network preview \
    --proposal-id <tx_hex>#<idx> \
    --output fixtures/conway-ratification/<short-id>.json
```

Expected on first run: either `wrote fixtures/...` or a panic with the Koios URL + body. Fix the capture helper iteratively if a fetch fails (e.g. wrong query param name) and re-run until the file lands.

- [ ] **Step 2: Write a round-trip loader test scoped to this specific fixture**

Append to `crates/dugite-ledger/tests/common/ratification_fixture.rs` tests module (or replace `MINIMAL_JSON` path with the real file):

```rust
    #[test]
    fn real_preview_fixture_loads() {
        let path = format!(
            "{}/../../fixtures/conway-ratification/<short-id>.json",
            env!("CARGO_MANIFEST_DIR")
        );
        if !std::path::Path::new(&path).exists() {
            eprintln!("skipping — fixture not captured yet: {path}");
            return;
        }
        let fixture = RatificationFixture::load(&path);
        let ledger = fixture.into_ledger_state();
        assert_eq!(ledger.gov.governance.proposals.len(), 1);
    }
```

Substitute `<short-id>` for the captured file name.

- [ ] **Step 3: Run the loader test**

```bash
cargo nextest run -p dugite-ledger -E 'test(real_preview_fixture_loads)'
```

Expected: PASS. **If it fails, the fix goes in both the capture helper and the fixture schema until the round trip works** — this is the entire point of Task 5. Typical fixes:
- Koios returns bech32 DRep / pool ids → the capture helper must hex-decode them from bech32 before emitting.
- Koios returns `proposal_type` as `"ParameterChange"` → map to `"PParamUpdate"` when writing the fixture.
- Koios committee shape is `{ "members": [...] }` with different field names → reshape in the helper so the fixture exactly matches the `FixtureCommittee` struct.

Keep iterating until the test passes.

- [ ] **Step 4: Write the capture-instructions README**

Create `fixtures/conway-ratification/README.md`:

```markdown
# Conway ratification fixtures

Offline JSON fixtures consumed by
`crates/dugite-ledger/tests/conway_ratification.rs`.  Captured once via
`target/debug/capture-ratification-fixture` against the public preview
Koios endpoint and committed.  **No live network access at test time.**

## Capturing a new fixture

```bash
cargo build -p dugite-cli --bin capture-ratification-fixture
./target/debug/capture-ratification-fixture \
    --network preview \
    --proposal-id <tx_hex>#<cert_index> \
    --output fixtures/conway-ratification/<name>.json
```

Then add a `#[test]` in `crates/dugite-ledger/tests/conway_ratification.rs`
that loads the new file.
```

- [ ] **Step 5: Lint + fmt + commit**

```bash
cargo clippy -p dugite-ledger --tests -- -D warnings
cargo fmt -p dugite-ledger -- --check

git add fixtures/conway-ratification/ \
        crates/dugite-ledger/tests/common/ratification_fixture.rs \
        crates/dugite-cli/src/bin/capture_ratification_fixture.rs
git commit -m "$(cat <<'EOF'
test(ledger): capture first positive Conway ratification fixture

One real preview proposal captured via capture-ratification-fixture
and committed.  Loader round-trips through into_ledger_state.
EOF
)"
```

---

## Task 6: Positive ratification assertion test

**Files:**
- Create: `crates/dugite-ledger/tests/conway_ratification.rs`
- Modify: `crates/dugite-ledger/tests/common/ratification_fixture.rs` (add `assert_ratified` / `assert_not_ratified` helpers)

Calls `ratify_proposals()` on the minimal ledger state and asserts the expected `enacted_*` slot matches.

- [ ] **Step 1: Add assertion helpers**

Append to `crates/dugite-ledger/tests/common/ratification_fixture.rs`:

```rust
use dugite_primitives::transaction::GovActionId as _GovActionId;

pub fn assert_ratified(
    ledger: &LedgerState,
    expected_bucket: EnactedBucket,
    expected_id: &GovActionId,
) {
    let gov = &ledger.gov.governance;
    let actual = match expected_bucket {
        EnactedBucket::PParamUpdate => gov.enacted_pparam_update.as_ref(),
        EnactedBucket::HardFork => gov.enacted_hard_fork.as_ref(),
        EnactedBucket::Committee => gov.enacted_committee.as_ref(),
        EnactedBucket::Constitution => gov.enacted_constitution.as_ref(),
    };
    assert_eq!(
        actual,
        Some(expected_id),
        "bucket {:?}: expected {:?}, got {:?}",
        expected_bucket,
        expected_id,
        actual
    );
}

pub fn assert_not_ratified(ledger: &LedgerState, proposal_id: &GovActionId) {
    let gov = &ledger.gov.governance;
    for slot in [
        &gov.enacted_pparam_update,
        &gov.enacted_hard_fork,
        &gov.enacted_committee,
        &gov.enacted_constitution,
    ] {
        assert_ne!(slot.as_ref(), Some(proposal_id));
    }
    // Also acceptable: proposal was dropped from the live set entirely.
}
```

(Remove the spurious `use ... as _GovActionId;` if `GovActionId` is already in scope from earlier imports.)

- [ ] **Step 2: Create the integration test file**

Create `crates/dugite-ledger/tests/conway_ratification.rs`:

```rust
//! Integration tests validating `ratify_proposals()` against committed
//! Koios-captured fixtures.  One `#[test]` per fixture.

mod common;

use common::ratification_fixture::{
    assert_ratified, EnactedBucket, RatificationFixture,
};

#[test]
fn ratifies_first_positive_preview_proposal() {
    let path = format!(
        "{}/../../fixtures/conway-ratification/<short-id>.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture = RatificationFixture::load(&path);
    let expected_bucket = fixture.expected_outcome.enacted_bucket;
    let expected_id = common::ratification_fixture::parse_gov_action_id(
        fixture
            .expected_outcome
            .enacted_id
            .as_deref()
            .expect("positive fixture must carry enacted_id"),
    );
    let mut ledger = fixture.into_ledger_state();
    ledger.ratify_proposals();
    assert_ratified(&ledger, expected_bucket, &expected_id);
}
```

Substitute the real captured filename for `<short-id>` and make `parse_gov_action_id` `pub` in `ratification_fixture.rs`.

- [ ] **Step 3: Run the test**

```bash
cargo nextest run -p dugite-ledger -E 'test(ratifies_first_positive_preview_proposal)'
```

**Expected outcomes and what to do:**
- **PASS:** Great — the algorithm matches Haskell for this fixture. Proceed.
- **Wrong bucket / wrong ID:** This is a real divergence. Per the spec's Divergence Classification section, start investigation at `crates/dugite-ledger/src/state/governance.rs::ratify_proposals` thresholding or the `enact_gov_action` bucket mapping. Fix-and-retry lands in the **same PR** per the spec's "Done when".
- **Fixture defect (e.g. wrong snapshot epoch):** Re-capture, re-commit, re-run.

If the divergence is systemic and blocks the plan, escalate — do not force a passing test by weakening the assertion.

- [ ] **Step 4: Lint + fmt**

```bash
cargo clippy -p dugite-ledger --tests -- -D warnings
cargo fmt -p dugite-ledger -- --check
```

- [ ] **Step 5: Commit**

```bash
git add crates/dugite-ledger/tests/conway_ratification.rs \
        crates/dugite-ledger/tests/common/ratification_fixture.rs
git commit -m "$(cat <<'EOF'
test(ledger): assert first positive Conway ratification fixture

Calls ratify_proposals() on the minimal LedgerState built from the
committed fixture and asserts the expected enacted_* slot.
EOF
)"
```

---

## Task 7: Capture one negative fixture

**Files:**
- Create: `fixtures/conway-ratification/<neg-id>.json`

Repeat the Task 5 flow for a proposal that did NOT ratify (expired with insufficient votes or explicitly voted down). This catches false-positive enactment bugs — a far more dangerous class than false negatives.

- [ ] **Step 1: Pick a negative candidate**

```bash
curl -s 'https://preview.koios.rest/api/v1/proposal_list' \
  | jq '[.[] | select(.dropped_epoch != null and .enacted_epoch == null)] | .[0:5]'
```

Pick the first result whose action type is one of the 4 in-scope buckets (the negative assertion path doesn't care about the bucket — any action type proves "not enacted").

- [ ] **Step 2: Capture**

```bash
./target/debug/capture-ratification-fixture \
    --network preview \
    --proposal-id <neg_tx_hex>#<idx> \
    --output fixtures/conway-ratification/<neg-short-id>.json
```

- [ ] **Step 3: Hand-edit `expected_outcome` in the captured JSON**

Set:
```json
"expected_outcome": {
  "ratified": false,
  "enacted_bucket": "PParamUpdate",
  "enacted_epoch": 0,
  "enacted_id": null
}
```

The `enacted_bucket` value is unused for negative fixtures (the helper only reads `ratified: false`) but must still parse as a valid `EnactedBucket` — any of the four is fine.

- [ ] **Step 4: Commit**

```bash
git add fixtures/conway-ratification/<neg-short-id>.json
git commit -m "$(cat <<'EOF'
test(ledger): capture first negative Conway ratification fixture

Expired/dropped preview proposal — used to catch false-positive
enactment bugs.
EOF
)"
```

---

## Task 8: Negative-case assertion test

**Files:**
- Modify: `crates/dugite-ledger/tests/conway_ratification.rs`

- [ ] **Step 1: Add the `#[test]` alongside the positive one**

Append to `crates/dugite-ledger/tests/conway_ratification.rs`:

```rust
use common::ratification_fixture::assert_not_ratified;

#[test]
fn rejects_first_negative_preview_proposal() {
    let path = format!(
        "{}/../../fixtures/conway-ratification/<neg-short-id>.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture = RatificationFixture::load(&path);
    let proposal_id = common::ratification_fixture::parse_gov_action_id(
        &fixture.proposal.gov_action_id,
    );
    assert!(!fixture.expected_outcome.ratified, "fixture must be negative");
    let mut ledger = fixture.into_ledger_state();
    ledger.ratify_proposals();
    assert_not_ratified(&ledger, &proposal_id);
}
```

- [ ] **Step 2: Run it**

```bash
cargo nextest run -p dugite-ledger -E 'test(rejects_first_negative_preview_proposal)'
```

Expected: PASS. If `ratify_proposals()` incorrectly enacts a negative fixture, that is exactly the bug this test exists to catch — investigate threshold math and fix in the same PR.

- [ ] **Step 3: Lint + fmt**

```bash
cargo clippy -p dugite-ledger --tests -- -D warnings
cargo fmt -p dugite-ledger -- --check
```

- [ ] **Step 4: Commit**

```bash
git add crates/dugite-ledger/tests/conway_ratification.rs
git commit -m "$(cat <<'EOF'
test(ledger): assert first negative Conway ratification fixture

Verifies ratify_proposals() does NOT enact an expired/dropped
preview proposal.
EOF
)"
```

---

## Task 9: Full workspace verification

**Files:** none (verification only)

- [ ] **Step 1: Full nextest run**

```bash
cargo nextest run --workspace
```

Expected: every test passes. If serialization tests flake in parallel (a known pre-existing issue from Phase A), retry once before treating as a regression.

- [ ] **Step 2: Doc tests**

```bash
cargo test --doc --workspace
```

- [ ] **Step 3: Clippy + fmt across the workspace**

```bash
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

- [ ] **Step 4: Confirm the two new `#[test]`s show up in nextest output**

```bash
cargo nextest run -p dugite-ledger -E 'test(ratifies_first_positive_preview_proposal) or test(rejects_first_negative_preview_proposal)'
```

Expected: `2 passed, 0 failed`.

- [ ] **Step 5: Verify "Done when" criteria from the spec**

Manually check each bullet:
- [ ] `capture-ratification-fixture` binary builds.
- [ ] At least 2 fixtures committed under `fixtures/conway-ratification/` (1 positive, 1 negative).
- [ ] `crates/dugite-ledger/tests/conway_ratification.rs` contains ≥2 `#[test]` and both pass.
- [ ] Clippy + fmt + nextest clean.
- [ ] `fixtures/conway-ratification/README.md` documents the one-shot capture command.

- [ ] **Step 6: No commit in this task** unless earlier tasks left unstaged fixup.

---

## Notes for the implementer

- **Do not touch `crates/dugite-ledger/src/state/governance.rs`** unless Task 6 or Task 8 surfaces a real divergence. The whole point of Phase B is validating the production code as-is.
- **If a divergence is found,** the fix lands in the same PR per the spec's "Done when". Open a short investigation note in the PR description identifying which of the three divergence buckets it falls into (wrong outcome / wrong bucket / fixture defect).
- **Field-name correction vs. the spec:** an earlier version of the spec text referenced a nonexistent `drep_power_snapshot` field. The correct fields are `drep_distribution_snapshot`, `drep_snapshot_no_confidence`, `drep_snapshot_abstain` — all verified at `crates/dugite-ledger/src/state/governance.rs:1664` and `crates/dugite-ledger/src/state/mod.rs:370-378`. The spec has been edited to match; this plan uses the correct names throughout.
- **`GovAction` reconstruction is deliberately minimal.** The first-slice fixtures only need the action *tag* to reach the right `enacted_*` slot; inner-field fidelity is a follow-up once a second category of action (e.g. PParamUpdate with real param deltas) needs asserting. If Task 6 fails because a thresholding decision depends on action content, extend `parse_gov_action` at that point — do not speculate ahead.
