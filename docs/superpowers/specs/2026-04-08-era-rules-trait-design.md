# Era Rules Trait — Ledger Era Dispatch Refactor

**Date:** 2026-04-08
**Status:** Approved (revised after Haskell cross-reference review)
**Scope:** `crates/dugite-ledger`

## Problem

The ledger's era-specific business rules are scattered across ~6 files with ~25 runtime `if protocol_version_major >= N` guards embedded in a monolithic `apply_block()` method (~1200 lines). The existing `eras/` module has empty marker structs (`ShelleyLedger`, `ConwayLedger`) that were intended to organize rules but never materialized. This makes it difficult to:

- **Audit correctness** — you can't look at one place and see "these are the Conway rules"
- **Add new eras** — requires scattering more `if` guards across the codebase
- **Test era-specific behavior** — must construct full `LedgerState` for every test
- **Verify against the Haskell spec** — the Haskell node has clean per-era STS rule impls; our flat structure doesn't map to it

## Design

Three interconnected changes:

1. **Decompose `LedgerState` into component sub-states** — enables granular `&mut` borrows
2. **Define an `EraRules` trait** — encapsulates all era-varying behavior
3. **Implement per-era rule structs** — with shared logic composed via helper functions

### 1. State Decomposition

Split the current ~50-field flat `LedgerState` into 5 component sub-states, aligned with the Haskell `NewEpochState`/`EpochState`/`UTxOState`/`CertState` hierarchy.

#### `UtxoState`

Haskell equivalent: `UTxOState`

| Field | Type | Current location |
|-------|------|-----------------|
| `utxo_set` | `UtxoSet` | `LedgerState.utxo_set` |
| `diff_seq` | `DiffSeq` | `LedgerState.diff_seq` |
| `epoch_fees` | `Lovelace` | `LedgerState.epoch_fees` |
| `pending_donations` | `Lovelace` | `LedgerState.pending_donations` |

#### `CertState`

Haskell equivalent: `CertState` (DState + PState)

| Field | Type | Current location |
|-------|------|-----------------|
| `delegations` | `Arc<HashMap<Hash32, Hash28>>` | `LedgerState.delegations` |
| `pool_params` | `Arc<HashMap<Hash28, PoolRegistration>>` | `LedgerState.pool_params` |
| `future_pool_params` | `HashMap<Hash28, PoolRegistration>` | `LedgerState.future_pool_params` |
| `pending_retirements` | `HashMap<Hash28, EpochNo>` | `LedgerState.pending_retirements` |
| `reward_accounts` | `Arc<HashMap<Hash32, Lovelace>>` | `LedgerState.reward_accounts` |
| `stake_key_deposits` | `HashMap<Hash32, u64>` | `LedgerState.stake_key_deposits` |
| `pool_deposits` | `HashMap<Hash28, u64>` | `LedgerState.pool_deposits` |
| `total_stake_key_deposits` | `u64` | `LedgerState.total_stake_key_deposits` |
| `pointer_map` | `HashMap<Pointer, Hash32>` | `LedgerState.pointer_map` |
| `stake_distribution` | `StakeDistributionState` | `LedgerState.stake_distribution` |
| `script_stake_credentials` | `HashSet<Hash32>` | `LedgerState.script_stake_credentials` |

#### `GovState`

Haskell equivalent: `ConwayGovState` / `GovState era`

| Field | Type | Current location |
|-------|------|-----------------|
| `governance` | `Arc<GovernanceState>` | `LedgerState.governance` |

Already well-isolated behind `Arc`. The wrapper struct provides the `&mut` boundary.

#### `ConsensusState`

Haskell equivalent: fields from `NewEpochState` + `ChainDepState`

| Field | Type | Current location |
|-------|------|-----------------|
| `evolving_nonce` | `Hash32` | `LedgerState.evolving_nonce` |
| `candidate_nonce` | `Hash32` | `LedgerState.candidate_nonce` |
| `epoch_nonce` | `Hash32` | `LedgerState.epoch_nonce` |
| `lab_nonce` | `Hash32` | `LedgerState.lab_nonce` |
| `last_epoch_block_nonce` | `Hash32` | `LedgerState.last_epoch_block_nonce` |
| `rolling_nonce` | `Hash32` | `LedgerState.rolling_nonce` |
| `first_block_hash_of_epoch` | `Option<Hash32>` | `LedgerState.first_block_hash_of_epoch` |
| `prev_epoch_first_block_hash` | `Option<Hash32>` | `LedgerState.prev_epoch_first_block_hash` |
| `epoch_blocks_by_pool` | `Arc<HashMap<Hash28, u64>>` | `LedgerState.epoch_blocks_by_pool` |
| `epoch_block_count` | `u64` | `LedgerState.epoch_block_count` |
| `opcert_counters` | `HashMap<Hash28, u64>` | `LedgerState.opcert_counters` |

#### `EpochState`

Haskell equivalent: `EpochState` + `SnapShots` + current/previous protocol parameters.

Protocol parameters live here because they change at epoch boundaries (via governance enactment or pre-Conway PP update proposals). In the Haskell node, `curPParams` lives inside `GovState` which is inside `UTxOState` — we place them in `EpochState` as the Rust-idiomatic equivalent, since `process_epoch_transition` is the method that mutates them and it already takes `&mut EpochState`.

| Field | Type | Current location |
|-------|------|-----------------|
| `snapshots` | `EpochSnapshots` | `LedgerState.snapshots` |
| `treasury` | `Lovelace` | `LedgerState.treasury` |
| `reserves` | `Lovelace` | `LedgerState.reserves` |
| `pending_reward_update` | `Option<PendingRewardUpdate>` | `LedgerState.pending_reward_update` |
| `pending_pp_updates` | `BTreeMap<EpochNo, Vec<...>>` | `LedgerState.pending_pp_updates` |
| `future_pp_updates` | `BTreeMap<EpochNo, Vec<...>>` | `LedgerState.future_pp_updates` |
| `needs_stake_rebuild` | `bool` | `LedgerState.needs_stake_rebuild` |
| `ptr_stake` | `HashMap<Pointer, u64>` | `LedgerState.ptr_stake` |
| `ptr_stake_excluded` | `bool` | `LedgerState.ptr_stake_excluded` |
| `protocol_params` | `ProtocolParameters` | `LedgerState.protocol_params` |
| `prev_protocol_params` | `ProtocolParameters` | `LedgerState.prev_protocol_params` |
| `prev_protocol_version_major` | `u64` | `LedgerState.prev_protocol_version_major` |
| `prev_d` | `f64` | `LedgerState.prev_d` |

**Rationale for `protocol_params` placement:** Governance enactment (`ParameterChange`, `HardForkInitiation`) modifies `protocol_params` during `process_epoch_transition`. If `protocol_params` were on the orchestrator level and passed read-only via `RuleContext`, the trait method could not modify it — a compile-time error. Placing it in `EpochState` makes it mutable within `process_epoch_transition` while still accessible read-only via `RuleContext` during per-tx processing.

#### Remaining on `LedgerState` (orchestrator)

Top-level coordination state — either immutable config or cross-cutting bookkeeping:

- `tip`, `era`, `pending_era_transition`, `epoch`
- `epoch_length`, `shelley_transition_epoch`, `byron_epoch_length`
- `slot_config`, `genesis_hash`, `genesis_delegates`, `update_quorum`
- `node_network`, `stability_window`, `randomness_stabilisation_window`, `stability_window_3kf`

```rust
pub struct LedgerState {
    // Component sub-states
    pub utxo: UtxoState,
    pub certs: CertState,
    pub gov: GovState,
    pub consensus: ConsensusState,
    pub epochs: EpochState,

    // Coordination
    pub tip: Tip,
    pub era: Era,
    pub pending_era_transition: Option<(Era, Era, EpochNo)>,
    pub epoch: EpochNo,
    pub epoch_length: u64,
    pub shelley_transition_epoch: u64,
    pub byron_epoch_length: u64,
    pub slot_config: SlotConfig,
    pub genesis_hash: Hash32,
    pub genesis_delegates: HashMap<Hash28, (Hash28, Hash32)>,
    pub update_quorum: u64,
    pub node_network: Option<NetworkId>,
    pub stability_window: u64,
    pub randomness_stabilisation_window: u64,
    pub stability_window_3kf: u64,
}
```

### 2. EraRules Trait

Stateless strategy trait. Implementations carry no mutable state — all state lives in the sub-state components.

```rust
/// Read-only context available to all era rules.
/// Assembled by the orchestrator before dispatching.
pub struct RuleContext<'a> {
    pub params: &'a ProtocolParameters,
    pub current_slot: u64,
    pub current_epoch: EpochNo,
    pub era: Era,
    pub slot_config: Option<&'a SlotConfig>,
    pub node_network: Option<NetworkId>,
    pub genesis_delegates: &'a HashMap<Hash28, (Hash28, Hash32)>,
    pub update_quorum: u64,
    pub epoch_length: u64,
    pub shelley_transition_epoch: u64,
    pub byron_epoch_length: u64,
    pub stability_window: u64,
    pub randomness_stabilisation_window: u64,
}
```

```rust
pub trait EraRules {
    // --- Block-level validation ---

    /// Validate block body constraints (ExUnit budgets, ref script sizes,
    /// body size check).
    ///
    /// Haskell: BBODY steps 1-2 (validateBlockBodySize, validateBlockBodyHash).
    /// Era-specific: Conway adds block-level ref script size limit (PV >= 9).
    fn validate_block_body(
        &self,
        block: &Block,
        ctx: &RuleContext,
        utxo: &UtxoState,
    ) -> Result<(), LedgerError>;

    // --- Transaction application ---

    /// Apply a single valid transaction (IsValid=true path).
    ///
    /// Implements the full LEDGER rule pipeline for the era. For Conway, this
    /// must execute in the following order (matching Haskell conwayLedgerTransition):
    ///
    ///   1. validateTreasuryValue — declared treasury matches actual
    ///   2. validateRefScriptSize — per-tx ref script size <= 200 KiB
    ///   3. validateWithdrawalsDelegated (PV >= 10) — withdrawal keys DRep-delegated
    ///   4. testIncompleteAndMissingWithdrawals (PV >= 10) — exact withdrawal amounts
    ///   5. updateDormantDRepExpiries / updateVotingDRepExpiries — DRep activity
    ///   6. drainAccounts — apply withdrawals to reward account balances
    ///   7. CERTS — process all certificates left-to-right
    ///   8. GOV — process votes and proposals
    ///   9. UTXOW — witness validation + UTXO phase-1 + UTXOS phase-2/mutation
    ///
    /// Steps 1-8 are PRE-UTXO and must execute before any UTxO state mutation.
    /// Earlier eras have a subset of these steps (Shelley: DELEGS + UTXOW only).
    fn apply_valid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoState,
        certs: &mut CertState,
        gov: &mut GovState,
        epochs: &mut EpochState,
    ) -> Result<UtxoDiff, LedgerError>;

    /// Apply an invalid transaction (IsValid=false, collateral consumption path).
    ///
    /// Haskell: UTXOS invalid path. Steps 1-8 of the LEDGER rule are entirely
    /// skipped. Only UTXOW runs, dispatching to UTXO → UTXOS collateral path:
    ///   - Evaluate Plutus scripts — require at least one to fail
    ///   - Consume collateral inputs
    ///   - Add collateral return output (Babbage+)
    ///   - Credit totalCollateral to fees
    ///
    /// Pre-Alonzo eras: this method should never be called (no IsValid concept).
    fn apply_invalid_tx(
        &self,
        tx: &Transaction,
        mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoState,
    ) -> Result<UtxoDiff, LedgerError>;

    // --- Epoch transitions ---

    /// Process an epoch boundary transition.
    ///
    /// This is intentionally cross-cutting — epoch boundaries touch all sub-states,
    /// matching Haskell's TICK/NEWEPOCH/EPOCH which takes the full NewEpochState.
    ///
    /// Haskell EPOCH sequence (Conway):
    ///   1.  SNAP — rotate snapshots (go←set, set←mark, mark←compute)
    ///   2.  POOLREAP — retire pools, return/sweep deposits
    ///   3.  Complete DRep pulser, extract ratification results
    ///   4.  Apply treasury withdrawals from enacted proposals
    ///   5.  proposalsApplyEnactment — remove enacted/expired proposals
    ///   6.  Return proposal deposits to returnAddr accounts
    ///   7.  Update ConwayGovState (committee, constitution, PParams)
    ///   8.  Update numDormantEpochs
    ///   9.  Prune expired committee members
    ///  10.  Apply donations + unclaimed rewards to treasury
    ///  11.  Recalculate utxosDeposited = totalObligation(certState, govState)
    ///  12.  HARDFORK — if enacted PV > current PV, trigger era transition
    ///  13.  setFreshDRepPulsingState — initialize new pulser
    ///
    /// Shelley EPOCH: SNAP + POOLREAP + PPUP (genesis-key-vote PP updates). No
    /// governance ratification, no DRep pulser, no treasury withdrawal proposals.
    ///
    /// Cross-state mutations by phase:
    ///   - SNAP: &mut EpochState (snapshots), reads CertState (delegations, pools)
    ///   - POOLREAP: &mut CertState (pool_params, delegations), &mut EpochState (treasury)
    ///   - Ratification/enactment: &mut GovState, &mut EpochState (protocol_params, treasury)
    ///   - Reward distribution: &mut CertState (reward_accounts), &mut EpochState (treasury, reserves)
    ///   - Nonce computation: &mut ConsensusState
    ///   - totalObligation: reads CertState + GovState, writes UtxoState (not currently tracked)
    fn process_epoch_transition(
        &self,
        new_epoch: EpochNo,
        ctx: &RuleContext,
        utxo: &mut UtxoState,
        certs: &mut CertState,
        gov: &mut GovState,
        epochs: &mut EpochState,
        consensus: &mut ConsensusState,
    ) -> Result<(), LedgerError>;

    // --- Consensus bookkeeping ---

    /// Evolve the nonce state after applying a block header.
    ///
    /// Byron (OBFT): evolving_nonce does NOT advance; lab_nonce = prev_hash.
    /// Shelley+ (Praos): evolving_nonce = H(evolving || vrf_output); lab_nonce = prev_hash.
    /// TPraos (Shelley-Alonzo, PV < 7): raw 64-byte VRF output.
    /// Praos (Babbage+, PV >= 7): blake2b-256("N" || vrf_result).
    /// Stability window: Babbage uses 3k/f, Conway uses 4k/f.
    fn evolve_nonce(
        &self,
        header: &BlockHeader,
        ctx: &RuleContext,
        consensus: &mut ConsensusState,
    );

    // --- Fee calculation ---

    /// Minimum fee for a transaction under this era's rules.
    ///
    /// Needs UTxO access because Conway's tiered ref-script fee requires
    /// resolving reference inputs to calculate totalRefScriptSize.
    fn min_fee(
        &self,
        tx: &Transaction,
        ctx: &RuleContext,
        utxo: &UtxoState,
    ) -> u64;

    // --- Era transition (TranslateEra) ---

    /// Handle hard fork state transformations when entering this era.
    ///
    /// Called when `block.era > self.era`. This is NOT a default empty impl —
    /// each era must explicitly handle its transition or return Ok(()) if no
    /// transformation is needed.
    ///
    /// Babbage → Conway (the most complex transition) must perform:
    ///   1. Pointer stake purge — drop all pointer-address delegations from CertState
    ///   2. VState creation — initialize governance.dreps, committee state from ConwayGenesis
    ///   3. VRF key hash map — scan all registered pools, build refcount map
    ///   4. ConwayGovState creation — initial committee, constitution, empty proposals
    ///   5. utxosDonation reset — zero out pending_donations
    ///   6. InstantStake recomputation — rebuild incremental stake from UTxO
    ///
    /// Shelley → Allegra: timelock script support (structural, no state change).
    /// Allegra → Mary: multi-asset value type (structural, no state change).
    /// Mary → Alonzo: collateral/phase-2 fields, deposit tracking (structural).
    /// Alonzo → Babbage: ref inputs/scripts, collateral return (structural).
    fn on_era_transition(
        &self,
        from_era: Era,
        ctx: &RuleContext,
        utxo: &mut UtxoState,
        certs: &mut CertState,
        gov: &mut GovState,
        consensus: &mut ConsensusState,
        epochs: &mut EpochState,
    ) -> Result<(), LedgerError>;

    // --- Witness requirements ---

    /// Compute the set of required VKey witnesses for a transaction.
    ///
    /// Shelley-Babbage: spending input keys + withdrawal keys + certificate keys
    ///   + requiredSigners.
    /// Conway adds: voter keys (DRep KeyHash voters, CC hot keys, SPO voters).
    fn required_witnesses(
        &self,
        tx: &Transaction,
        ctx: &RuleContext,
        utxo: &UtxoState,
        certs: &CertState,
        gov: &GovState,
    ) -> HashSet<Hash28>;
}
```

#### Era dispatch

Use an enum for dispatch instead of `dyn` trait objects. `apply_block` is a hot path (called millions of times during sync), and enum dispatch enables the compiler to monomorphize and inline the per-era implementations:

```rust
pub enum EraRulesImpl {
    Byron(ByronRules),
    Shelley(ShelleyRules),
    Alonzo(AlonzoRules),
    Babbage(BabbageRules),
    Conway(ConwayRules),
}

impl EraRulesImpl {
    pub fn for_era(era: Era) -> Self {
        match era {
            Era::Byron => Self::Byron(ByronRules),
            Era::Shelley | Era::Allegra | Era::Mary => Self::Shelley(ShelleyRules),
            Era::Alonzo => Self::Alonzo(AlonzoRules),
            Era::Babbage => Self::Babbage(BabbageRules),
            Era::Conway => Self::Conway(ConwayRules),
        }
    }
}
```

Each method on `EraRulesImpl` delegates to the inner type via a match. This is zero-cost (the match compiles to a jump table).

Shelley/Allegra/Mary share one impl — Allegra adds timelock scripts and Mary adds multi-asset, but these are handled by the common validation layer checking field presence in the transaction (timelock validity interval, multi-asset in value), not by separate era dispatch. The ledger rules (fee calculation, UTxO mutation, certificate processing) are identical.

### 3. Era Implementations and Shared Logic

File structure:

```
crates/dugite-ledger/src/eras/
├── mod.rs          — EraRules trait, RuleContext, EraRulesImpl enum, dispatch
├── common.rs       — Shared helpers (Shelley base validation, UTxO mutation, cert processing)
├── byron.rs        — ByronRules (already substantive)
├── shelley.rs      — ShelleyRules (Shelley/Allegra/Mary)
├── alonzo.rs       — AlonzoRules (Plutus, collateral, phase-2)
├── babbage.rs      — BabbageRules (ref inputs, inline datums)
└── conway.rs       — ConwayRules (governance, tiered fees, DReps)
```

Composition pattern — era impls compose shared helpers rather than inheriting:

```rust
impl EraRules for ConwayRules {
    fn apply_valid_tx(
        &self, tx: &Transaction, mode: BlockValidationMode,
        ctx: &RuleContext,
        utxo: &mut UtxoState, certs: &mut CertState,
        gov: &mut GovState, epochs: &mut EpochState,
    ) -> Result<UtxoDiff, LedgerError> {
        // --- Conway LEDGER steps 1-4: pre-UTXO validation ---
        if mode == BlockValidationMode::ValidateAll {
            conway::validate_treasury_value(tx, epochs)?;             // Step 1
            conway::validate_per_tx_ref_script_size(tx, utxo, ctx)?;  // Step 2
            conway::validate_withdrawals_delegated(tx, certs, ctx)?;  // Step 3 (PV >= 10)
        }

        // --- Conway LEDGER steps 5-6: pre-UTXO state mutations ---
        conway::update_drep_expiries(tx, gov, ctx);                   // Step 5
        common::drain_withdrawal_accounts(tx, certs)?;                // Step 6

        // --- Conway LEDGER step 7: CERTS ---
        common::process_shelley_certs(tx, ctx, certs)?;
        conway::process_governance_certs(tx, ctx, certs, gov)?;

        // --- Conway LEDGER step 8: GOV ---
        conway::process_proposals_and_votes(tx, ctx, gov)?;

        // --- Conway LEDGER step 9: UTXOW + UTXO + UTXOS ---
        if mode == BlockValidationMode::ValidateAll {
            let mut errors = common::validate_shelley_base(tx, utxo, ctx);
            errors.extend(common::validate_alonzo_scripts(tx, utxo, ctx));
            errors.extend(conway::validate_conway_specific(tx, utxo, ctx, gov));
            if !errors.is_empty() {
                return Err(/* ... */);
            }
        }
        let diff = common::apply_utxo_changes(tx, utxo)?;
        utxo.epoch_fees += tx.body.fee;

        Ok(diff)
    }

    fn apply_invalid_tx(
        &self, tx: &Transaction, mode: BlockValidationMode,
        ctx: &RuleContext, utxo: &mut UtxoState,
    ) -> Result<UtxoDiff, LedgerError> {
        // Collateral consumption path — no certs, no governance, no withdrawals
        common::apply_collateral_consumption(tx, utxo, ctx)
    }
}
```

Intra-era protocol version differences (e.g., Conway PV9 bootstrap vs PV10) are handled via `ctx.params.protocol_version_major` checks localized within the Conway module.

### 4. Orchestrator

`apply_block()` becomes a thin dispatcher:

```rust
impl LedgerState {
    pub fn apply_block(
        &mut self,
        block: &Block,
        mode: BlockValidationMode,
    ) -> Result<(), LedgerError> {
        let rules = EraRulesImpl::for_era(block.era);
        let ctx = RuleContext::from_state(self, block);

        // 1. Connectivity check (era-agnostic)
        self.verify_block_connects(block)?;

        // 2. Era transition (TranslateEra)
        if block.era > self.era {
            rules.on_era_transition(
                self.era, &ctx,
                &mut self.utxo, &mut self.certs,
                &mut self.gov, &mut self.consensus,
                &mut self.epochs,
            )?;
            self.pending_era_transition = Some((self.era, block.era, self.epoch));
        }

        // 3. Epoch transitions
        let block_epoch = EpochNo(self.epoch_of_slot(block.slot().0));
        while self.epoch < block_epoch {
            let next = EpochNo(self.epoch.0 + 1);
            let epoch_rules = EraRulesImpl::for_era(self.era);
            epoch_rules.process_epoch_transition(
                next, &ctx,
                &mut self.utxo, &mut self.certs,
                &mut self.gov, &mut self.epochs,
                &mut self.consensus,
            )?;
            self.epoch = next;
        }

        // 4. Block-level validation
        rules.validate_block_body(block, &ctx, &self.utxo)?;

        // 5. Apply transactions (IsValid dispatch)
        let mut block_diff = UtxoDiff::new();
        for tx in &block.transactions {
            let diff = if tx.is_valid {
                rules.apply_valid_tx(
                    tx, mode, &ctx,
                    &mut self.utxo, &mut self.certs,
                    &mut self.gov, &mut self.epochs,
                )?
            } else {
                rules.apply_invalid_tx(tx, mode, &ctx, &mut self.utxo)?
            };
            block_diff.merge(diff);
        }

        // 6. Consensus bookkeeping
        rules.evolve_nonce(&block.header, &ctx, &mut self.consensus);

        // 7. Finalize
        self.tip = block.tip();
        self.era = block.era;
        self.utxo.diff_seq.push(block.slot(), *block.hash(), block_diff);

        Ok(())
    }
}
```

**Note on `RuleContext` construction:** `RuleContext::from_state` borrows `self.epochs.protocol_params` immutably. This is safe because:
- During tx application (step 5), `protocol_params` is not mutated (only `epochs: &mut EpochState` is passed for treasury validation, but params are read-only).
- During epoch transition (step 3), `RuleContext` must be reconstructed after each transition since `protocol_params` may change.

## Invariants

These invariants must be maintained across the refactor. They are enforced by the STS rules collectively and serve as correctness assertions for testing.

### Deposit Conservation (epoch boundary)

```
totalDeposited == sum(stake_key_deposits)
               + sum(pool_deposits)
               + sum(drep.deposit for drep in governance.dreps)
               + sum(proposal.deposit for proposal in governance.proposals)
```

Re-established at every epoch boundary (step 11 of Conway EPOCH). Already implemented at `epoch.rs:690-718`. Each `EraRules::process_epoch_transition` impl must maintain this.

### Value Conservation (per transaction)

```
consumed == produced

consumed = sum(utxo[input].value for input in tx.inputs)
         + sum(withdrawal amounts)
         + sum(deposit refunds for deregistration/pool-retire/drep-unreg certs)

produced = sum(output.value)
         + fee
         + sum(new deposits for registration/pool-reg/drep-reg/proposal certs)
         + net minted multi-asset (ADA component always 0)
```

Enforced in UTXO Phase-1 validation (`ValueNotConservedUTxO`).

### Reward Conservation (per epoch)

```
deltaT + deltaR + sum(rewards_credited) + deltaF == 0
```

Where `deltaT` is treasury change, `deltaR` is reserves change, `sum(rewards_credited)` is total credits to reward accounts, `deltaF` is fee pot contribution. Enforced by `applyRUpd`.

### Pool Stake Distribution Lag (2-epoch)

```
nesPd (used for VRF leader checks in epoch N) ==
    ssStakeMarkPoolDistr computed from epoch N-1's mark snapshot
```

Staking changes in epoch N don't affect leader eligibility until epoch N+2.

### Certificate and Transaction Ordering

```
cert[j+1] sees CertState after cert[j] is applied (left-to-right)
tx[i+1] sees UTxO/CertState/GovState after tx[i] is applied (sequential)
```

Enables within-tx register-then-delegate and within-block output spending chains.

### Governance Action Ancestry

```
if proposal.prev_action_id = Some(id):
    id must be the current enacted root or a descendant for this governance purpose
```

Enforced by the GOV rule (`InvalidPrevGovActionId`). Each governance purpose (PParamUpdate, HardFork, Committee, Constitution) maintains an independent chain rooted at the last enacted action.

### CC Resignation Permanence

```
Once CommitteeMemberResigned ∈ committee_resigned[cold_cred]:
    ConwayAuthCommitteeHotKey for cold_cred must be rejected
```

Enforced by `ConwayCommitteeHasPreviouslyResigned` in GOVCERT.

### VRF Key Uniqueness (PV >= 11)

```
Each VRF key hash maps to exactly one pool (refcount = 1)
```

Enforced by `VRFKeyHashAlreadyRegistered` in the POOL rule.

## Serialization Strategy

`LedgerState` uses bincode serialization for snapshots. Restructuring fields into sub-states would break existing snapshots if we naively derive `Serialize`/`Deserialize` on the new nested structure.

**Approach:** Define a dedicated `LedgerStateSnapshot` flat struct matching the current bincode field order. Implement `From<&LedgerState> for LedgerStateSnapshot` and `From<LedgerStateSnapshot> for LedgerState` conversions. Snapshot serialization always goes through this flat wire format.

This cleanly separates the in-memory organization (sub-states for granular borrows) from the on-disk format (flat struct for backward compatibility). Adding or removing fields from a sub-state doesn't accidentally break the snapshot format — changes must be made explicitly in `LedgerStateSnapshot`.

## Migration Strategy

Incremental, each step independently testable and committable:

1. **Extract sub-states** — introduce `UtxoState`, `CertState`, `ConsensusState`, `EpochState`, `GovState` structs with `protocol_params` in `EpochState`. Add `LedgerStateSnapshot` for serialization. Initially re-groupings within `LedgerState` with forwarding accessors. All tests stay green.

2. **Define the trait** — add `EraRules` trait with method signatures, `EraRulesImpl` enum. Implement `ByronRules` first (already isolated). `apply_block()` dispatches Byron through the enum, everything else stays as-is.

3. **Extract shared helpers into `common.rs`** — lift reusable logic (Shelley base validation, UTxO mutation, certificate processing, withdrawal draining, collateral consumption) out of `apply_block()` and `validation/mod.rs` into standalone functions that take sub-state references.

4. **Implement `ShelleyRules`** — using the common helpers. Switch Shelley/Allegra/Mary dispatch.

5. **Implement `AlonzoRules`, `BabbageRules`** — each building on common helpers + era-specific modules. `AlonzoRules` adds Phase-2 Plutus evaluation and the IsValid=false collateral path. `BabbageRules` adds reference inputs, inline datums, collateral return.

6. **Implement `ConwayRules`** — the largest: full 9-step LEDGER pipeline, governance (GOV rule with 13 proposal + 6 vote checks), DRep certificates (GOVCERT), tiered ref-script fees, DRep activity tracking, TranslateEra from Babbage.

7. **Clean up** — remove dead code from old monolithic `apply_block()`, retire inline era checks, delete empty `ShelleyLedger`/`ConwayLedger` marker structs.

## Testing Strategy

- **Existing test suite** validates correctness at every migration step — this is a pure refactor, no behavior changes
- **Per-era unit tests** — each `EraRules` impl gets tests that construct only the sub-states it needs (the primary testability improvement)
- **Invariant assertions** — add debug-mode assertions for deposit conservation, value conservation, and reward conservation that run after each block/epoch
- **Integration** — full `apply_block()` round-trip tests continue to work unchanged
- **Conformance tests** — the existing `tests/conformance/` suite validates against Haskell cardano-ledger behavior
- **TranslateEra tests** — dedicated tests for each era transition, particularly Babbage→Conway (pointer stake purge, VState creation, VRF key map population)

## Risks

- **Serialization compatibility** — mitigated by the `LedgerStateSnapshot` flat wire format approach (see Serialization Strategy above). The in-memory sub-state layout is fully decoupled from the on-disk format.
- **Performance** — enum dispatch is zero-cost (compiled to jump table). Field access through sub-states is identical to flat access after inlining. Benchmark before/after to confirm no regression in the block application hot path.
- **Merge conflicts** — this touches the core ledger path. Should be done on a dedicated branch with minimal concurrent changes to `state/apply.rs`.
- **`RuleContext` lifetime** — `protocol_params` is borrowed from `EpochState`. During epoch transitions, `RuleContext` must be reconstructed after each transition since params may change. The orchestrator handles this by constructing the context inside the epoch loop.
