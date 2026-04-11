//! Offline fixture schema used by `conway_ratification.rs` integration tests.
//!
//! Fields mirror the JSON schema defined in the Phase B design spec.  No
//! runtime behavior beyond deserialization + conversion into a `LedgerState`
//! (see `into_ledger_state`).

// Task 2 consumes most fields; `pparams`, `total_drep_stake`, `total_spo_stake`,
// `expected_outcome.*`, and committee `min_size` remain dead until Task 6
// wires up ratification assertions and real GovAction reconstruction.
#![allow(dead_code)]
#![allow(clippy::enum_variant_names)]

use dugite_ledger::state::{GovernanceState, LedgerState, ProposalState, StakeSnapshot};
use dugite_primitives::credentials::Credential;
use dugite_primitives::hash::{Hash28, Hash32, TransactionHash};
use dugite_primitives::protocol_params::ProtocolParameters;
use dugite_primitives::time::EpochNo;
use dugite_primitives::transaction::{
    Anchor, GovAction, GovActionId, ProposalProcedure, Rational, Vote, Voter, VotingProcedure,
};
use dugite_primitives::value::Lovelace;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

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

/// Decode a hex string to raw bytes, panicking on error with context.
fn decode_hex_bytes(hex_str: &str, ctx: &str) -> Vec<u8> {
    if !hex_str.len().is_multiple_of(2) {
        panic!("invalid hex for {ctx}: odd length ({hex_str})");
    }
    (0..hex_str.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&hex_str[i..i + 2], 16)
                .unwrap_or_else(|e| panic!("invalid hex byte for {ctx} at {i}: {e}"))
        })
        .collect()
}

/// Parse a `"<32-byte-hex>#<u32>"` action id string into a `GovActionId`.
pub fn parse_gov_action_id(s: &str) -> GovActionId {
    let (hash_hex, idx_str) = s
        .split_once('#')
        .unwrap_or_else(|| panic!("malformed gov_action_id (missing '#'): {s}"));
    let transaction_id: TransactionHash = Hash32::from_hex(hash_hex)
        .unwrap_or_else(|e| panic!("invalid gov action tx hash hex {hash_hex}: {e}"));
    let action_index: u32 = idx_str
        .parse()
        .unwrap_or_else(|e| panic!("invalid gov action index {idx_str}: {e}"));
    GovActionId {
        transaction_id,
        action_index,
    }
}

/// Parse a 32-byte-hex string into a `Hash32`, panicking on error with context.
fn parse_hash32(hex_str: &str, ctx: &str) -> Hash32 {
    Hash32::from_hex(hex_str)
        .unwrap_or_else(|e| panic!("invalid Hash32 hex for {ctx} ({hex_str}): {e}"))
}

/// Parse a 28-byte-hex string into a `Hash28`, panicking on error with context.
fn parse_hash28(hex_str: &str, ctx: &str) -> Hash28 {
    Hash28::from_hex(hex_str)
        .unwrap_or_else(|e| panic!("invalid Hash28 hex for {ctx} ({hex_str}): {e}"))
}

impl RatificationFixture {
    /// Load a fixture from a JSON file, panicking on IO or parse errors.
    pub fn load(path: &str) -> Self {
        let contents = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read fixture file {path}: {e}"));
        serde_json::from_str(&contents)
            .unwrap_or_else(|e| panic!("failed to parse fixture {path}: {e}"))
    }

    /// Build a minimal `LedgerState` positioned at the fixture's `pparams_epoch`
    /// with exactly the fields `ratify_proposals()` reads populated:
    ///
    /// * `gov.governance.proposals` + `votes_by_action`
    /// * `gov.governance.committee_*` (hot keys, expiration, resigned, threshold)
    /// * `gov.governance.drep_distribution_snapshot`, `drep_snapshot_no_confidence`, and `drep_snapshot_abstain` (so `build_drep_power_cache` takes the snapshot path rather than the live-state fallback)
    /// * `epochs.snapshots.set.pool_stake` (SPO stake read by `ratify_proposals`)
    /// * `gov.governance.enacted_*` roots from `parent_enacted`
    ///
    /// `ratification_snapshot` is intentionally left `None` so live state is used.
    /// `vote_delegations` is left empty so the snapshot DRep path is taken.
    pub fn into_ledger_state(self) -> LedgerState {
        let mut ledger = LedgerState::new(ProtocolParameters::mainnet_defaults());
        ledger.epoch = EpochNo(self.pparams_epoch);

        // Parse the proposal ID once — used as the key in both `proposals`
        // and `votes_by_action`.
        let action_id = parse_gov_action_id(&self.proposal.gov_action_id);

        // Build the proposal procedure. For first-slice fixtures we only need
        // a tag-level-correct action; use `InfoAction` as a placeholder for any
        // of the in-scope tags (PParamUpdate / HardFork / Committee /
        // Constitution). Reconstructing the inner fields is deferred to a
        // later task.
        let return_addr = decode_hex_bytes(&self.proposal.return_addr_hex, "return_addr_hex");
        let anchor = self
            .proposal
            .anchor
            .as_ref()
            .map(|a| Anchor {
                url: a.url.clone(),
                data_hash: parse_hash32(&a.data_hash, "proposal anchor data_hash"),
            })
            .unwrap_or_else(|| Anchor {
                url: String::new(),
                data_hash: Hash32::ZERO,
            });
        // TODO(task-6): reconstruct real GovAction from self.proposal.action JSON.
        // For the first-slice fixtures only the action *tag* routes the proposal
        // to the right `enacted_*` slot, so InfoAction is a placeholder that
        // Task 6 must replace before ratification tests can assert on anything
        // tag-sensitive.
        let _ = &self.proposal.action;
        let procedure = ProposalProcedure {
            deposit: Lovelace(self.proposal.deposit),
            return_addr,
            gov_action: GovAction::InfoAction,
            anchor,
        };

        // Count votes by value into yes/no/abstain tallies (u64 counts — we
        // don't have per-vote stake at this level, stake comes from the
        // DRep/SPO snapshot maps).
        let mut yes_votes: u64 = 0;
        let mut no_votes: u64 = 0;
        let mut abstain_votes: u64 = 0;
        for v in &self.votes {
            match v.vote {
                FixtureVoteValue::Yes => yes_votes += 1,
                FixtureVoteValue::No => no_votes += 1,
                FixtureVoteValue::Abstain => abstain_votes += 1,
            }
        }

        let proposal_state = ProposalState {
            procedure,
            proposed_epoch: EpochNo(self.proposed_epoch),
            expires_epoch: EpochNo(self.proposal.expiration),
            yes_votes,
            no_votes,
            abstain_votes,
        };

        // Build the (Voter, VotingProcedure) list for `votes_by_action`.
        let votes_vec: Vec<(Voter, VotingProcedure)> = self
            .votes
            .iter()
            .map(|fv| {
                let voter = match fv.voter_type {
                    FixtureVoterType::ConstitutionalCommitteeHotKeyHash => {
                        Voter::ConstitutionalCommittee(Credential::VerificationKey(parse_hash28(
                            &fv.voter_id,
                            "cc hot key hash voter",
                        )))
                    }
                    FixtureVoterType::ConstitutionalCommitteeHotScriptHash => {
                        Voter::ConstitutionalCommittee(Credential::Script(parse_hash28(
                            &fv.voter_id,
                            "cc hot script hash voter",
                        )))
                    }
                    FixtureVoterType::DRepKeyHash => Voter::DRep(Credential::VerificationKey(
                        parse_hash28(&fv.voter_id, "drep key hash voter"),
                    )),
                    FixtureVoterType::DRepScriptHash => Voter::DRep(Credential::Script(
                        parse_hash28(&fv.voter_id, "drep script hash voter"),
                    )),
                    FixtureVoterType::StakePoolKeyHash => Voter::StakePool(
                        parse_hash28(&fv.voter_id, "stake pool key hash voter").to_hash32_padded(),
                    ),
                };
                let vote = match fv.vote {
                    FixtureVoteValue::Yes => Vote::Yes,
                    FixtureVoteValue::No => Vote::No,
                    FixtureVoteValue::Abstain => Vote::Abstain,
                };
                (voter, VotingProcedure { vote, anchor: None })
            })
            .collect();

        // Mutate the inner GovernanceState (Arc-wrapped in GovSubState).
        {
            let gov: &mut GovernanceState = Arc::make_mut(&mut ledger.gov.governance);

            gov.proposals.insert(action_id.clone(), proposal_state);
            gov.votes_by_action.insert(action_id.clone(), votes_vec);

            // Committee state
            for member in &self.committee.members {
                let cold = parse_hash32(&member.cold_key, "committee cold key");
                gov.committee_expiration
                    .insert(cold, EpochNo(member.expiration));
                if let Some(hot_hex) = &member.hot_key {
                    let hot = parse_hash32(hot_hex, "committee hot key");
                    gov.committee_hot_keys.insert(cold, hot);
                }
            }
            for resigned_hex in &self.committee.resigned {
                let cold = parse_hash32(resigned_hex, "committee resigned cold key");
                gov.committee_resigned.insert(cold, None);
            }
            gov.committee_threshold = Some(Rational {
                numerator: self.committee.threshold.numerator,
                denominator: self.committee.threshold.denominator,
            });

            // DRep power snapshot (keys are 32-byte typed credential hashes).
            // Leaving `vote_delegations` empty ensures `build_drep_power_cache`
            // uses the snapshot fields rather than the live-state fallback.
            for (drep_hex, stake) in &self.drep_power {
                let cred_hash = parse_hash32(drep_hex, "drep_power credential hash");
                gov.drep_distribution_snapshot.insert(cred_hash, *stake);
            }
            gov.drep_snapshot_no_confidence = self.drep_no_confidence;
            gov.drep_snapshot_abstain = self.drep_abstain;

            // Enacted roots (parent_enacted) — each field is optional.
            gov.enacted_pparam_update = self
                .parent_enacted
                .pparam_update
                .as_deref()
                .map(parse_gov_action_id);
            gov.enacted_hard_fork = self
                .parent_enacted
                .hard_fork
                .as_deref()
                .map(parse_gov_action_id);
            gov.enacted_committee = self
                .parent_enacted
                .committee
                .as_deref()
                .map(parse_gov_action_id);
            gov.enacted_constitution = self
                .parent_enacted
                .constitution
                .as_deref()
                .map(parse_gov_action_id);
        }

        // SPO stake — `ratify_proposals()` reads `epochs.snapshots.set.pool_stake`.
        // Build a minimal "set" snapshot at `pparams_epoch` containing just the
        // pool_stake entries from the fixture; other snapshot fields stay empty.
        let mut set_snapshot = StakeSnapshot::empty(EpochNo(self.pparams_epoch));
        for (pool_hex, stake) in &self.spo_stake {
            let pool_id = parse_hash28(pool_hex, "spo_stake pool id");
            set_snapshot.pool_stake.insert(pool_id, Lovelace(*stake));
        }
        ledger.epochs.snapshots.set = Some(set_snapshot);

        ledger
    }
}

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
        "bucket {expected_bucket:?}: expected {expected_id:?}, got {actual:?}",
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

#[cfg(test)]
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
    assert_eq!(
        fixture.expected_outcome.enacted_bucket,
        EnactedBucket::PParamUpdate
    );
    assert!(fixture.expected_outcome.ratified);
}

#[test]
fn minimal_fixture_builds_ledger_state_with_one_proposal() {
    let fixture: RatificationFixture = serde_json::from_str(MINIMAL_JSON).expect("parse");
    let ledger = fixture.into_ledger_state();
    assert_eq!(ledger.gov.governance.proposals.len(), 1);
    assert!(ledger.gov.governance.drep_distribution_snapshot.is_empty());
    assert_eq!(ledger.gov.governance.drep_snapshot_no_confidence, 0);
}

#[test]
fn real_preview_fixture_loads() {
    let path = format!(
        "{}/../../fixtures/conway-ratification/preview-pparam-1096.json",
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
