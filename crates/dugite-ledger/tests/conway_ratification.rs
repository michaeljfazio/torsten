//! Integration tests validating `ratify_proposals()` against committed
//! Koios-captured fixtures.  One `#[test]` per fixture.

mod common;

use common::ratification_fixture::{assert_ratified, parse_gov_action_id, RatificationFixture};

// The fixture is a real preview ParameterChange proposal routed through
// `ratify_proposals()`.  `drep_power` / `spo_stake` remain stubbed (captured
// thresholds are bypassed via bootstrap phase + zero SPO security threshold in
// the loader), but the CC approval path uses real CC voter hot-key hashes
// from the captured votes.  See fixtures/conway-ratification/README.md and
// `reconstruct_gov_action` in tests/common/ratification_fixture.rs.
#[test]
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
