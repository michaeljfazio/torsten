# Ledger Lead Agent Memory

## Reference Files

- [reward-formula-validation.md](reward-formula-validation.md) — Cross-validation of Torsten reward calculation vs Koios on-chain data (preview epoch 1235). Includes exact formulas, verified invariants, and the RUPD timing difference vs Haskell.
- [n2c-hash32-padding-convention.md](n2c-hash32-padding-convention.md) — The ledger uses Hash32 (32-byte zero-padded) as HashMap keys for 28-byte Blake2b-224 hashes. All N2C wire output for credentials/pool IDs must truncate to 28 bytes. Lists all affected LedgerState fields and the fix pattern.
- [cascade-failure-treasury-committee.md](cascade-failure-treasury-committee.md) — Root cause and fix for the slot 107229218 cascade failure: TreasuryValueMismatch and UnelectedCommitteeMember hard-returns caused UTxO corruption and network fork. Key invariant: never hard-return Err for confirmed on-chain blocks on ledger-state-divergence checks.
- [slow-path-rollback-utxo-store.md](slow-path-rollback-utxo-store.md) — Slow-path rollback bug: re-attaching the pre-rollback UTxO store left stale entries. Fixed to open a fresh store from the "ledger" LSM snapshot. Key: LSM snapshot and bincode ledger snapshot are always saved together.
