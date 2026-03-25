---
name: Preview pool expected block rate (pool1d92w...)
description: Expected block production rate for the test pool on preview testnet, and how to interpret leader check metrics
type: project
---

Pool ID (hex): `6954ec11cf7097a693721104139b96c54e7f3e2a8f9e7577630f7856`
Pool ID (bech32): `pool1d92wcyw0wzt6dymjzyzp8xukc488703237082amrpau9vgadcnk`

**Stake parameters (epoch 1241, Set snapshot):**
- pool_stake: 1,040,501,700,234 lovelace (~1.04 ADA billion)
- active_stake: 1,242,319,321,820,827 lovelace (~1.24 quadrillion ADA)
- sigma (relative stake): 0.083755%

**Expected block production:**
- phi_f(sigma) = 1 - (1 - 0.05)^0.00083755 = 4.296e-5 per slot
- Expected blocks per epoch (86400 slots): ~3.71
- Expected blocks per hour (3600 slots): ~0.155
- Average slot gap between blocks: ~23,270 slots (~6.5 hours)

**Interpreting leader_checks_total:**
On preview (1-second slots), `leader_checks_total` ≈ seconds spent at tip.
316 checks = ~5 minutes at tip. P(0 elections in 316 checks) = 98.65%. Completely normal.

To reliably expect at least 1 block, need ~23,000 checks (6.5 hours) at tip.

**Historical note:** Pool shows 0 blocks in epochs 1234-1239 on Koios, even with ~1.01T ADA stake. This is consistent: with sigma=0.081% and only ~5 epoch-hours at tip, the probability of 0 blocks in a single epoch is ~(1-phi)^86400 ≈ 2.3% — rare but not impossible over 6 epochs.
