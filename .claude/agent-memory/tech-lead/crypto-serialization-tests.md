---
name: serialization test coverage
description: Coverage gaps found and filled in dugite-serialization (2026-03-20)
type: project
---

Added 133 tests in `crates/dugite-serialization/tests/comprehensive_coverage.rs`.

**Key findings about the public API boundary:**
- `encode_protocol_param_update`, `encode_redeemer`, `encode_gov_action`, `encode_drep`, `encode_voter`, `encode_voting_procedure`, `encode_proposal_procedure` are NOT re-exported from the crate public API — they are `pub(crate)` inside sub-modules.
- Integration tests must exercise these through the public wrapper functions: `encode_witness_set` for redeemers, `encode_transaction_body` for gov actions / proposal procedures.
- `encode_protocol_param_update` is only reachable via `GovAction::ParameterChange` inside a proposal procedure inside a transaction body.

**Key type facts:**
- `Relay::SingleHostAddr.ipv4` is `Option<[u8; 4]>` (array, not Vec)
- `ProtocolParamUpdate.min_fee_ref_script_cost_per_byte` is `Option<u64>` (not `Option<Rational>`); the encoder converts it to a Rational internally when writing key 30
- `ProtocolParamUpdate.dvt_motion_no_confidence` is actually `dvt_no_confidence`

**PPU extraction pattern** for testing via body:
```rust
// Embed PPU in ParameterChange → proposal_procedure → body
// Navigate: body[key=20] → array(1) → pp array(4) → [skip deposit, skip return_addr, gov_action]
// gov_action: array(4) [tag=0, null, ppu_map, null]
// ppu_map starts at dec.position() after consuming tag+null
```

**Why:** `encode_protocol_param_update` cannot be called from integration tests because it is `pub(crate)`. The unit tests inside `encode/mod.rs` do call it via `use protocol_params::encode_protocol_param_update` which only works within the same crate.
