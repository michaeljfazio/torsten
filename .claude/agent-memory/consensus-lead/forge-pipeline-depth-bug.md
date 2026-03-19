---
name: Forge ticker disabled during pipelined sync
description: In pipelined+pool mode, forge_ticker is guarded by pipeline_depth<=1, so no forge checks happen during the hours-long initial sync phase
type: project
---

In `crates/torsten-node/src/node/sync.rs`, the pipelined+pool sync path (the most common path on preview testnet) guards the forge ticker branch with:

```rust
_ = forge_ticker.tick(), if self.block_producer.is_some() && pipeline_depth <= 1 => {
```

During initial sync, `pipeline_depth = 150`. At tip, it is reduced to 1. This means the forge ticker is **completely disabled** until the node fully syncs to tip.

**Why:** This was presumably to avoid spurious forge attempts during bulk sync, but the consequence is that if the node spends many hours syncing (e.g., after a Mithril import), zero leader checks happen during that window.

**How to apply:** When investigating low forge rates, check whether the node was at tip for a meaningful fraction of the reported soak window. `leader_checks_total` metric directly measures time-at-tip in seconds (since each check = ~1 second on preview). If `leader_checks_total` is small relative to the reported soak duration, the node was mostly syncing, not at tip.

The sequential (non-pipelined) mode and the pipelined-without-pool mode do NOT have this `pipeline_depth <= 1` guard — they fire the forge ticker even during sync. This is the inconsistency.
