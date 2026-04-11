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

## Known stub fields (Task 5 → Task 6 follow-up)

The first-slice helper leaves several fields as zero/empty placeholders so a
fresh capture round-trips through the loader without manual editing:

- `drep_power`, `drep_no_confidence`, `drep_abstain`, `total_drep_stake`
  (per-DRep enumeration deferred to Task 6)
- `spo_stake`, `total_spo_stake` (bech32 → hex pool id decode deferred to
  Task 6)
- `committee` (transformation of `committee_info` to canonical shape deferred
  to Task 6)
- `pparams` (left as `{}` — `ratify_proposals` reads thresholds from
  `LedgerState::protocol_params`, not the fixture blob)
- `parent_enacted` (recursive prev_action_id capture deferred to Task 6)

These stubs are sufficient for `proposals.len() == 1` round-trip assertion
but **must be filled in before any real ratification outcome assertion in
Task 6**.
