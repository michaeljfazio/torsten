//! Integration tests validating `ratify_proposals()` against committed
//! Koios-captured fixtures.  One `#[test]` per fixture.

mod common;

use common::ratification_fixture::{assert_ratified, parse_gov_action_id, RatificationFixture};

// Ignored: the captured fixture currently stubs `drep_power`, `spo_stake`,
// `committee`, and `parent_enacted` (see fixtures/conway-ratification/README.md)
// AND the loader replaces the captured `action` with an `InfoAction`
// placeholder (see TODO(task-6) in tests/common/ratification_fixture.rs).
// `ratify_proposals` therefore cannot route this proposal to
// `enacted_pparam_update`.  Un-ignore once a follow-up populates the snapshot
// fields and reconstructs a real `PParamUpdate` GovAction from
// `proposal_description`.
#[test]
#[ignore = "needs Task 6 follow-up: populate snapshot stubs + reconstruct PParamUpdate GovAction"]
fn ratifies_first_positive_preview_proposal() {
    let path = format!(
        "{}/../../fixtures/conway-ratification/preview-pparam-1096.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture = RatificationFixture::load(&path);
    let expected_bucket = fixture.expected_outcome.enacted_bucket;
    let expected_id = parse_gov_action_id(
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
