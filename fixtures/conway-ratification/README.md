# Conway ratification fixtures

Offline JSON fixtures consumed by
`crates/dugite-ledger/tests/conway_ratification.rs`. Captured once via
`target/debug/capture-ratification-fixture` against the public preview Koios
endpoint and committed. **No live network access at test time.**

## Capturing a new fixture

```bash
cargo build -p dugite-cli --bin capture-ratification-fixture
./target/debug/capture-ratification-fixture \
    --network preview \
    --proposal-id <tx_hex>#<proposal_index> \
    --output fixtures/conway-ratification/<name>.json
```

The helper queries `proposal_list`, `proposal_voting_summary`,
`proposal_votes`, `pool_voting_power_history`, `committee_info`, and
`epoch_params`, then transforms the raw Koios responses into the canonical
`RatificationFixture` JSON shape that the test loader consumes.

After capture, add a `#[test]` in
`crates/dugite-ledger/tests/conway_ratification.rs` that loads the new file.

## Stubbed snapshot fields

The capture helper leaves several fields as zero/empty placeholders.  The
positive test (`ratifies_first_positive_preview_proposal`) bypasses them by
running in Conway bootstrap phase (protocol version 9 → DRep thresholds
auto-pass) with the SPO Security-group threshold zeroed in the loader:

- `drep_power`, `drep_no_confidence`, `drep_abstain`, `total_drep_stake`
  — ignored in bootstrap phase
- `spo_stake`, `total_spo_stake` — ignored because the loader zeros
  `pvt_pp_security_group`
- `pparams` — left as `{}`; `ratify_proposals` reads thresholds from
  `LedgerState::protocol_params`, not the fixture blob

The **committee members** and **parent_enacted.PParamUpdate** fields must be
filled in manually after capture (the helper can't derive them from the
Koios `proposal_list` row alone):

- `committee.members[].hot_key` is the 28-byte CC hot credential hash with a
  type byte suffix (`01` for script, `00` for key-hash) padded to 32 bytes —
  matches `Credential::to_typed_hash32`
- `parent_enacted.PParamUpdate` must equal the proposal's own
  `prev_action_id` so `prev_action_as_expected` threads correctly
