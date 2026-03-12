# Block Validation Modes in cardano-node

## Two-Level Validation Architecture

Haskell cardano-node has TWO distinct validation levels, not a "sync mode" flag:

### 1. `apply` vs `reapply` (Consensus layer)
- **`tickThenApply`** = full validation: `applyBlockLedgerResult` (STS.ValidateAll) + `validateHeader` (full crypto checks)
- **`tickThenReapply`** = skip validation: `reapplyBlockLedgerResult` (STS.ValidateNone) + `revalidateHeader` (skip crypto checks)

### 2. When each is used
- **Replay from snapshot** (LedgerDB init): ALWAYS uses `tickThenReapply` (`initReapplyBlock`)
  - File: `Storage/LedgerDB/API.hs` -> `replayStartingWith` -> `initReapplyBlock`
  - V1: `Storage/LedgerDB/V1.hs` line 109
  - V2: `Storage/LedgerDB/V2.hs` line 93
- **New blocks from network** (chain selection): uses `mkAps` which decides per-block:
  - First time seeing a block -> `ApplyVal`/`ApplyRef` -> `tickThenApply` (FULL validation)
  - Previously applied block -> `ReapplyVal`/`ReapplyRef` -> `tickThenReapply` (SKIP validation)
  - File: `Storage/LedgerDB/Forker.hs` lines 349-362
- **`reapplyThenPushNOW`** (post-chain-selection): uses `tickThenReapply`
  - File: `Storage/LedgerDB/V2.hs` line 215

## What `reapply` skips

### Header validation (`revalidateHeader` vs `validateHeader`)
- File: `Consensus/HeaderValidation.hs`
- `validateHeader` calls `updateChainDepState` -> full VRF + KES + opcert verification
- `revalidateHeader` calls `reupdateChainDepState` -> NO crypto verification, just state update
- Envelope checks (block number, slot, prev hash) are done in both but as assertions only in revalidate

### Praos crypto (`reupdateChainDepState` vs `updateChainDepState`)
- File: `Protocol/Praos.hs`
- `updateChainDepState` (line 474): calls `validateKESSignature` + `validateVRFSignature`, then `reupdateChainDepState`
- `reupdateChainDepState` (line 501): ONLY updates nonces, opcert counters, last slot -- NO crypto checks at all
- During reapply: VRF proof NOT verified, KES signature NOT verified, opcert Ed25519 signature NOT verified

### Ledger rules (STS.ValidateNone)
- `reapplyBlockLedgerResult` calls `applyBlockLedgerResultWithValidation STS.ValidateNone`
- STS.ValidateNone causes the small-steps framework to skip predicate failures in STS rules
- For Shelley/Conway: BBODY transition runs with no validation (no UTxO checks, no script execution)
- File: `Shelley/Ledger/Ledger.hs` lines 602-618

## Key Design Insight
There is NO "bulk sync mode" or "initial sync" flag. The distinction is purely structural:
- Blocks from ImmutableDB (already persisted) are trusted -> `reapply`
- New blocks from network are untrusted -> `apply` (unless previously applied in same session)
- The `prevApplied` set tracks which blocks were validated in the current session

## GSM (Genesis State Machine)
- File: `Node/GsmState.hs`
- States: PreSyncing -> Syncing -> CaughtUp
- GSM is used for peer management (ChainSync jumping, historicity checks), NOT for validation policy
- Does NOT affect whether blocks are validated or not
