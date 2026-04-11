//! Offline fixture schema used by `conway_ratification.rs` integration tests.
//!
//! Fields mirror the JSON schema defined in the Phase B design spec.  No
//! runtime behavior beyond deserialization + conversion into a `LedgerState`
//! (see `into_ledger_state`).

// Task 1 only exercises deserialization — most fields are read by the
// `into_ledger_state` conversion that lands in Task 2.
#![allow(dead_code)]
#![allow(clippy::enum_variant_names)]

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
